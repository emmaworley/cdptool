//! Parser and serializer for the CDP (CHUMP) container format.
//!
//! CDP files use a tag-based binary structure to store Trainz game assets.
//! The format starts with a 16-byte header (`ACS$` magic, version, reserved,
//! body length), followed by a tree of typed tags.
//!
//! # Tag types
//!
//! | Byte | Type      | Payload |
//! |------|-----------|---------|
//! | 0x00 | Container | Nested child tags |
//! | 0x01 | Integer   | One or more `i32` values |
//! | 0x02 | Float     | One or more `f32` values |
//! | 0x03 | String    | Null-terminated UTF-8 |
//! | 0x04 | Binary    | Raw bytes (usually compressed file data) |
//! | 0x05 | Null      | Empty |
//! | 0x0D | KUID      | 8-byte asset identifier |

use crate::error::CdpError;
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::Cursor;

/// A parsed CDP document (header + tag tree).
pub struct CdpDocument {
    pub version: u32,
    pub reserved: u32,
    pub tags: Vec<CdpTag>,
}

/// A single CDP tag. The container format supports 7 types.
#[derive(Debug, Clone)]
pub enum CdpTag {
    Container {
        name: String,
        children: Vec<CdpTag>,
    },
    Integer {
        name: String,
        values: Vec<i32>,
    },
    Float {
        name: String,
        values: Vec<f32>,
    },
    String {
        name: String,
        value: String,
    },
    Binary {
        name: String,
        data: Vec<u8>,
    },
    Null {
        name: String,
    },
    Kuid {
        name: String,
        user_id: u32,
        content_id: i32,
    },
}

impl CdpTag {
    /// The tag's name field (every tag variant carries one).
    pub fn name(&self) -> &str {
        match self {
            Self::Container { name, .. }
            | Self::Integer { name, .. }
            | Self::Float { name, .. }
            | Self::String { name, .. }
            | Self::Binary { name, .. }
            | Self::Null { name, .. }
            | Self::Kuid { name, .. } => name,
        }
    }

    /// The wire-format type byte for this tag variant.
    pub fn tag_type_byte(&self) -> u8 {
        match self {
            Self::Container { .. } => 0x00,
            Self::Integer { .. } => 0x01,
            Self::Float { .. } => 0x02,
            Self::String { .. } => 0x03,
            Self::Binary { .. } => 0x04,
            Self::Null { .. } => 0x05,
            Self::Kuid { .. } => 0x0D,
        }
    }

    /// Pretty-print the tag tree at the given indentation depth.
    pub fn display(&self, depth: usize) -> String {
        let indent = "  ".repeat(depth);
        match self {
            Self::Container { name, children } => {
                let mut s = format!("{indent}[CONTAINER] {name}\n");
                for child in children {
                    s += &child.display(depth + 1);
                }
                s
            }
            Self::Integer { name, values } => {
                if values.len() == 1 {
                    format!("{indent}[INT] {name} = {}\n", values[0])
                } else {
                    format!("{indent}[INT[]] {name} = {values:?}\n")
                }
            }
            Self::Float { name, values } => {
                if values.len() == 1 {
                    format!("{indent}[FLOAT] {name} = {}\n", values[0])
                } else {
                    format!("{indent}[FLOAT[]] {name} = {values:?}\n")
                }
            }
            Self::String { name, value } => {
                format!("{indent}[STRING] {name} = \"{value}\"\n")
            }
            Self::Binary { name, data } => {
                format!("{indent}[BINARY] {name} ({} bytes)\n", data.len())
            }
            Self::Null { name } => {
                format!("{indent}[NULL] {name}\n")
            }
            Self::Kuid {
                name,
                user_id,
                content_id,
            } => {
                let flags = (user_id >> 22) & 0x3;
                let uid = user_id & 0x3FFFFF;
                if flags == 0 {
                    format!("{indent}[KUID] {name} = <kuid:{uid}:{content_id}>\n")
                } else {
                    format!("{indent}[KUID] {name} = <kuid{flags}:{uid}:{content_id}>\n")
                }
            }
        }
    }
}

// --- Parsing ---

fn read_u32(data: &[u8], offset: usize) -> u32 {
    Cursor::new(&data[offset..])
        .read_u32::<LittleEndian>()
        .unwrap()
}

fn read_i32(data: &[u8], offset: usize) -> i32 {
    Cursor::new(&data[offset..])
        .read_i32::<LittleEndian>()
        .unwrap()
}

fn read_f32(data: &[u8], offset: usize) -> f32 {
    Cursor::new(&data[offset..])
        .read_f32::<LittleEndian>()
        .unwrap()
}

fn parse_tags(data: &[u8], mut offset: usize, end: usize) -> Vec<CdpTag> {
    let mut tags = Vec::new();
    while offset + 6 <= end {
        let tag_len = read_u32(data, offset) as usize;
        let tag_start = offset;
        offset += 4;

        let name_len = data[offset] as usize;
        offset += 1;

        let name = if name_len > 0 {
            let s = std::string::String::from_utf8_lossy(&data[offset..offset + name_len - 1])
                .to_string();
            offset += name_len;
            s
        } else {
            std::string::String::new()
        };

        if offset >= end {
            break;
        }
        let tag_type = data[offset];
        offset += 1;

        let value_end = tag_start + 4 + tag_len;
        if value_end > end {
            break;
        }

        let value_data = &data[offset..value_end];

        let tag = match tag_type {
            0x00 => CdpTag::Container {
                name,
                children: parse_tags(data, offset, value_end),
            },
            0x01 => {
                let mut values = Vec::new();
                let mut pos = 0;
                while pos + 4 <= value_data.len() {
                    values.push(read_i32(value_data, pos));
                    pos += 4;
                }
                CdpTag::Integer { name, values }
            }
            0x02 => {
                let mut values = Vec::new();
                let mut pos = 0;
                while pos + 4 <= value_data.len() {
                    values.push(read_f32(value_data, pos));
                    pos += 4;
                }
                CdpTag::Float { name, values }
            }
            0x03 => {
                let s = value_data.split(|&b| b == 0).next().unwrap_or(b"");
                CdpTag::String {
                    name,
                    value: std::string::String::from_utf8_lossy(s).to_string(),
                }
            }
            0x04 => CdpTag::Binary {
                name,
                data: value_data.to_vec(),
            },
            0x05 => CdpTag::Null { name },
            0x0D => {
                let user_id = if value_data.len() >= 4 {
                    read_u32(value_data, 0)
                } else {
                    0
                };
                let content_id = if value_data.len() >= 8 {
                    read_i32(value_data, 4)
                } else {
                    0
                };
                CdpTag::Kuid {
                    name,
                    user_id,
                    content_id,
                }
            }
            _ => {
                // Unknown tag type: store as binary to preserve round-tripping
                CdpTag::Binary {
                    name,
                    data: value_data.to_vec(),
                }
            }
        };

        tags.push(tag);
        offset = value_end;
    }
    tags
}

/// Parse a CDP file from raw bytes.
pub fn parse(data: &[u8]) -> Result<CdpDocument, CdpError> {
    if data.len() < 16 || &data[0..4] != b"ACS$" {
        return Err(CdpError::InvalidMagic);
    }
    let version = read_u32(data, 4);
    let reserved = read_u32(data, 8);
    let tags = parse_tags(data, 16, data.len());
    Ok(CdpDocument {
        version,
        reserved,
        tags,
    })
}

// --- Serialization ---

fn serialize_tag(tag: &CdpTag) -> Vec<u8> {
    let name = tag.name();
    let tag_type = tag.tag_type_byte();

    let payload: Vec<u8> = match tag {
        CdpTag::Container { children, .. } => children.iter().flat_map(serialize_tag).collect(),
        CdpTag::Integer { values, .. } => values.iter().flat_map(|v| v.to_le_bytes()).collect(),
        CdpTag::Float { values, .. } => values.iter().flat_map(|v| v.to_le_bytes()).collect(),
        CdpTag::String { value, .. } => {
            let mut v: Vec<u8> = value.as_bytes().to_vec();
            v.push(0);
            v
        }
        CdpTag::Binary { data, .. } => data.clone(),
        CdpTag::Null { .. } => Vec::new(),
        CdpTag::Kuid {
            user_id,
            content_id,
            ..
        } => {
            let mut v = user_id.to_le_bytes().to_vec();
            v.extend_from_slice(&content_id.to_le_bytes());
            v
        }
    };

    // Tag header: 4-byte length, 1-byte name_len, name bytes + null, 1-byte type
    let name_section_len = if name.is_empty() { 0 } else { name.len() + 1 };
    let tag_len = 1 + name_section_len + 1 + payload.len();

    let mut out = Vec::with_capacity(4 + tag_len);
    out.extend_from_slice(&(tag_len as u32).to_le_bytes());
    if name.is_empty() {
        out.push(0);
    } else {
        out.push((name.len() + 1) as u8);
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    out.push(tag_type);
    out.extend_from_slice(&payload);
    out
}

/// Serialize a CDP document to bytes.
pub fn serialize(doc: &CdpDocument) -> Vec<u8> {
    let body: Vec<u8> = doc.tags.iter().flat_map(serialize_tag).collect();
    let mut out = Vec::with_capacity(16 + body.len());
    out.extend_from_slice(b"ACS$");
    out.extend_from_slice(&doc.version.to_le_bytes());
    out.extend_from_slice(&doc.reserved.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Recursively collect all Binary tags (files) from the tag tree.
pub fn collect_files(tags: &[CdpTag]) -> Vec<(&str, &[u8])> {
    let mut files = Vec::new();
    for tag in tags {
        match tag {
            CdpTag::Binary { name, data } => {
                files.push((name.as_str(), data.as_slice()));
            }
            CdpTag::Container { children, .. } => {
                files.extend(collect_files(children));
            }
            _ => {}
        }
    }
    files
}
