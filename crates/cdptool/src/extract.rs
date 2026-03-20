//! Structured extraction of CDP archives.
//!
//! Produces a flat list of [`ExtractedFile`] entries that mirror the CDP's
//! nested container layout. Each asset gets a `config.json` capturing all
//! non-file metadata, and its files are placed in paths that follow the
//! `files` container's subdirectory structure.
//!
//! Both the CLI and WASM frontends use this module so their output is
//! identical.

use crate::cdp::{CdpDocument, CdpTag};
use crate::error::CdpError;
use crate::lzss;
use serde_json::{Map, Value};

/// A single entry to be written during extraction.
pub struct ExtractedFile {
    /// Path relative to the output root (e.g. `assets/<kuid>/art/thumb.jpg`).
    pub path: String,
    /// Decompressed file contents.
    pub data: Vec<u8>,
}

/// Extract all assets from a parsed CDP document.
///
/// Returns one [`ExtractedFile`] per entry: a `config.json` for each asset
/// followed by its decompressed files in their original directory structure.
pub fn extract_all(doc: &CdpDocument) -> Result<Vec<ExtractedFile>, CdpError> {
    let mut out = Vec::new();

    for tag in &doc.tags {
        if let CdpTag::Container { name, children } = tag
            && name == "assets"
        {
            for asset in children {
                if let CdpTag::Container {
                    name: asset_name,
                    children: asset_children,
                } = asset
                {
                    let folder = format!("assets/{asset_name}/");

                    let config = build_config_json(asset_name, asset_children)?;
                    out.push(ExtractedFile {
                        path: format!("{folder}config.json"),
                        data: config.into_bytes(),
                    });

                    extract_files_recursive(asset_children, &folder, &mut out)?;
                }
            }
        }
    }
    Ok(out)
}

/// Like [`extract_all`] but returns entries lazily as `(path, compressed_blob)`
/// pairs without decompressing. The caller is responsible for calling
/// [`lzss::decompress`] on each blob. Used by the WASM frontend for
/// streaming extraction.
pub fn collect_pending(doc: &CdpDocument) -> Result<Vec<(String, Vec<u8>)>, CdpError> {
    let mut out = Vec::new();

    for tag in &doc.tags {
        if let CdpTag::Container { name, children } = tag
            && name == "assets"
        {
            for asset in children {
                if let CdpTag::Container {
                    name: asset_name,
                    children: asset_children,
                } = asset
                {
                    let folder = format!("assets/{asset_name}/");

                    let config = build_config_json(asset_name, asset_children)?;
                    out.push((
                        format!("{folder}config.json"),
                        make_stored_blob(config.as_bytes()),
                    ));

                    collect_file_blobs(asset_children, &folder, &mut out);
                }
            }
        }
    }
    Ok(out)
}

// --- Config JSON generation ---

/// Convert a single CDP tag to a JSON key-value pair.
///
/// Returns `None` for Binary tags (these become extracted files instead).
fn tag_to_json(tag: &CdpTag) -> Option<(String, Value)> {
    match tag {
        CdpTag::Container { name, children } => {
            let mut map = Map::new();
            for child in children {
                if let Some((k, v)) = tag_to_json(child) {
                    map.insert(k, v);
                }
            }
            Some((name.clone(), Value::Object(map)))
        }
        CdpTag::String { name, value } => Some((name.clone(), Value::String(value.clone()))),
        CdpTag::Integer { name, values } => {
            let v = if values.len() == 1 {
                Value::Number(values[0].into())
            } else {
                Value::Array(values.iter().map(|&n| Value::Number(n.into())).collect())
            };
            Some((name.clone(), v))
        }
        CdpTag::Float { name, values } => {
            let to_num = |f: f32| {
                serde_json::Number::from_f64(f as f64)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            };
            let v = if values.len() == 1 {
                to_num(values[0])
            } else {
                Value::Array(values.iter().map(|&f| to_num(f)).collect())
            };
            Some((name.clone(), v))
        }
        CdpTag::Kuid {
            name,
            user_id,
            content_id,
        } => Some((
            name.clone(),
            Value::String(format_kuid(*user_id, *content_id)),
        )),
        CdpTag::Null { name } => Some((name.clone(), Value::Null)),
        CdpTag::Binary { .. } => None,
    }
}

/// Build the `config.json` for one asset.
///
/// Includes all tags except `files` (extracted separately) and
/// `compression` (structural). Tag names are used as JSON keys verbatim.
fn build_config_json(asset_name: &str, children: &[CdpTag]) -> Result<String, CdpError> {
    let mut map = Map::new();
    map.insert(
        "_container".to_string(),
        Value::String(asset_name.to_string()),
    );
    map.insert(
        "_generated-by".to_string(),
        Value::String("https://github.com/emmaworley/cdptool".to_string()),
    );
    for child in children {
        match child {
            CdpTag::Container { name, .. } if name == "files" => continue,
            CdpTag::String { name, .. } if name == "compression" => continue,
            _ => {}
        }
        if let Some((k, v)) = tag_to_json(child) {
            map.insert(k, v);
        }
    }
    serde_json::to_string_pretty(&Value::Object(map))
        .map_err(|e| CdpError::JsonSerialize(e.to_string()))
}

fn format_kuid(user_id: u32, content_id: i32) -> String {
    let flags = (user_id >> 22) & 0x3;
    let uid = user_id & 0x3FFFFF;
    if flags == 0 {
        format!("<kuid:{uid}:{content_id}>")
    } else {
        format!("<kuid{flags}:{uid}:{content_id}>")
    }
}

// --- File collection ---

/// Find the `files` container and recursively extract+decompress its entries.
fn extract_files_recursive(
    children: &[CdpTag],
    prefix: &str,
    out: &mut Vec<ExtractedFile>,
) -> Result<(), CdpError> {
    for tag in children {
        if let CdpTag::Container {
            name,
            children: files_children,
        } = tag
            && name == "files"
        {
            decompress_tree(files_children, prefix, out)?;
        }
    }
    Ok(())
}

/// Recursively walk the files container tree, decompressing each Binary.
fn decompress_tree(
    tags: &[CdpTag],
    prefix: &str,
    out: &mut Vec<ExtractedFile>,
) -> Result<(), CdpError> {
    for tag in tags {
        match tag {
            CdpTag::Binary { name, data } if data.len() >= 4 => {
                // Defense in depth: the length guard above ensures this succeeds,
                // but use `ok()` + `continue` instead of `unwrap()` for robustness.
                let Some(size_bytes) = data[0..4].try_into().ok() else {
                    continue;
                };
                let uncomp_size = u32::from_le_bytes(size_bytes) as usize;
                let decompressed = lzss::decompress(&data[4..], uncomp_size)?;
                out.push(ExtractedFile {
                    path: format!("{prefix}{name}"),
                    data: decompressed,
                });
            }
            CdpTag::Container { name, children } => {
                decompress_tree(children, &format!("{prefix}{name}/"), out)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Collect compressed blobs without decompressing (for streaming/lazy extraction).
fn collect_file_blobs(children: &[CdpTag], prefix: &str, out: &mut Vec<(String, Vec<u8>)>) {
    for tag in children {
        if let CdpTag::Container {
            name,
            children: files_children,
        } = tag
            && name == "files"
        {
            collect_blobs_recursive(files_children, prefix, out);
        }
    }
}

fn collect_blobs_recursive(tags: &[CdpTag], prefix: &str, out: &mut Vec<(String, Vec<u8>)>) {
    for tag in tags {
        match tag {
            CdpTag::Binary { name, data } if data.len() >= 4 => {
                // Defense in depth: skip blobs where the size prefix can't be read.
                let Some(size_bytes) = data[0..4].try_into().ok() else {
                    continue;
                };
                let _uncomp_size: u32 = u32::from_le_bytes(size_bytes);
                out.push((format!("{prefix}{name}"), data.clone()));
            }
            CdpTag::Container { name, children } => {
                collect_blobs_recursive(children, &format!("{prefix}{name}/"), out);
            }
            _ => {}
        }
    }
}

/// Make a stored-mode LZSS blob from raw bytes (for synthetic entries like config.json).
fn make_stored_blob(data: &[u8]) -> Vec<u8> {
    let mut b = (data.len() as u32).to_le_bytes().to_vec();
    b.push(0x00); // mode=0, level=0
    b.extend_from_slice(data);
    b
}
