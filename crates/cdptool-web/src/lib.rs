//! WASM bindings for cdptool, exposing CDP pack/unpack to JavaScript.

use cdptool::cdp::{self as cdpmod, CdpDocument, CdpTag};
use cdptool::lzss;
use wasm_bindgen::prelude::*;

fn err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Parse a CDP file and extract all compressed files.
///
/// Returns a JS `Array` of `[name: string, data: Uint8Array]` pairs.
#[wasm_bindgen]
pub fn extract_cdp(cdp_bytes: &[u8]) -> Result<JsValue, JsValue> {
    let doc = cdpmod::parse(cdp_bytes).map_err(err)?;
    let files = cdpmod::collect_files(&doc.tags);
    let result = js_sys::Array::new();

    for (name, blob) in &files {
        if blob.len() < 4 {
            continue;
        }
        let uncomp_size = u32::from_le_bytes(blob[0..4].try_into().unwrap()) as usize;
        let decompressed = lzss::decompress(&blob[4..], uncomp_size).map_err(err)?;

        let pair = js_sys::Array::new();
        pair.push(&JsValue::from_str(name));
        pair.push(&js_sys::Uint8Array::from(decompressed.as_slice()).into());
        result.push(&pair);
    }
    Ok(result.into())
}

/// Compress files into a CDP archive.
///
/// Takes a JS `Array` of `[name: string, data: Uint8Array]` pairs.
/// Returns the serialized CDP as a `Uint8Array`.
#[wasm_bindgen]
pub fn create_cdp(files_js: &JsValue) -> Result<Vec<u8>, JsValue> {
    let files_array: &js_sys::Array = files_js
        .dyn_ref()
        .ok_or_else(|| err("expected array of [name, data] pairs"))?;

    let mut file_tags: Vec<CdpTag> = Vec::new();
    for i in 0..files_array.length() {
        let pair: js_sys::Array = files_array
            .get(i)
            .dyn_into()
            .map_err(|_| err("each element must be [name, data]"))?;
        let name: String = pair
            .get(0)
            .as_string()
            .ok_or_else(|| err("file name must be a string"))?;
        let data_js: js_sys::Uint8Array = pair
            .get(1)
            .dyn_into()
            .map_err(|_| err("file data must be Uint8Array"))?;
        let raw_data = data_js.to_vec();
        let uncomp_size = raw_data.len() as u32;

        let compressed = lzss::compress(&raw_data, 2, 9).map_err(err)?;
        let mut blob = uncomp_size.to_le_bytes().to_vec();
        blob.extend_from_slice(&compressed);

        file_tags.push(CdpTag::Binary { name, data: blob });
    }

    let doc = CdpDocument {
        version: 1,
        reserved: 0,
        tags: vec![
            CdpTag::Container {
                name: "assets".into(),
                children: vec![CdpTag::Container {
                    name: "asset".into(),
                    children: vec![
                        CdpTag::String {
                            name: "compression".into(),
                            value: "LZSS".into(),
                        },
                        CdpTag::Container {
                            name: "files".into(),
                            children: file_tags,
                        },
                    ],
                }],
            },
            CdpTag::String {
                name: "kind".into(),
                value: "archive".into(),
            },
            CdpTag::Integer {
                name: "package-version".into(),
                values: vec![1],
            },
        ],
    };
    Ok(cdpmod::serialize(&doc))
}

/// Parse a CDP file and return its tag tree as human-readable text.
#[wasm_bindgen]
pub fn info_cdp(cdp_bytes: &[u8]) -> Result<String, JsValue> {
    let doc = cdpmod::parse(cdp_bytes).map_err(err)?;
    let mut output = String::new();
    for tag in &doc.tags {
        output.push_str(&tag.display(0));
    }
    Ok(output)
}
