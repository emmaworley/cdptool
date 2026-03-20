use cdptool::cdp as cdpmod;
use cdptool::cdp::CdpTag;
use cdptool::extract;
use cdptool::lzss;

/// Build a minimal valid CDP document with the given files (name → raw bytes).
fn build_test_cdp(files: &[(&str, &[u8])]) -> Vec<u8> {
    let file_tags: Vec<CdpTag> = files
        .iter()
        .map(|(name, data)| {
            let compressed = lzss::compress(data, 2, 9).unwrap();
            let mut blob = (data.len() as u32).to_le_bytes().to_vec();
            blob.extend_from_slice(&compressed);
            CdpTag::Binary {
                name: name.to_string(),
                data: blob,
            }
        })
        .collect();

    let doc = cdpmod::CdpDocument {
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
        ],
    };
    cdpmod::serialize(&doc)
}

/// Extract all binary payloads from a CDP blob, decompress each, return
/// a vec of (name, decompressed_bytes).
fn extract_all(cdp_data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let doc = cdpmod::parse(cdp_data).unwrap();
    let files = cdpmod::collect_files(&doc.tags);
    files
        .iter()
        .filter(|(_, blob)| blob.len() >= 4)
        .map(|(name, blob)| {
            let uncomp_size = u32::from_le_bytes(blob[0..4].try_into().unwrap()) as usize;
            let data = lzss::decompress(&blob[4..], uncomp_size).unwrap();
            (name.to_string(), data)
        })
        .collect()
}

#[test]
fn parse_serialize_round_trip() {
    let pattern = [0xDE, 0xAD, 0xBE, 0xEF].repeat(256);
    let files: Vec<(&str, &[u8])> = vec![
        ("hello.txt", b"Hello, CDP world!"),
        ("zeros.bin", &[0u8; 512]),
        ("pattern.bin", &pattern),
    ];
    let cdp_data = build_test_cdp(&files);

    // Parse and re-serialize: must be byte-identical.
    let doc = cdpmod::parse(&cdp_data).unwrap();
    let reserialized = cdpmod::serialize(&doc);
    assert_eq!(cdp_data, reserialized, "parse→serialize not byte-identical");
}

#[test]
fn full_cdp_round_trip() {
    let original_files: Vec<(&str, Vec<u8>)> = vec![
        ("small.bin", vec![42u8; 100]),
        ("medium.bin", (0..4096).map(|i| (i % 256) as u8).collect()),
        (
            "large.bin",
            (0..50_000).map(|i| ((i * 37 + 13) % 256) as u8).collect(),
        ),
        ("repeated.bin", vec![0xAA; 10_000]),
    ];

    let cdp_data = build_test_cdp(
        &original_files
            .iter()
            .map(|(n, d)| (*n, d.as_slice()))
            .collect::<Vec<_>>(),
    );

    let extracted = extract_all(&cdp_data);
    assert_eq!(extracted.len(), original_files.len());

    for ((orig_name, orig_data), (ext_name, ext_data)) in
        original_files.iter().zip(extracted.iter())
    {
        assert_eq!(orig_name, ext_name);
        assert_eq!(orig_data, ext_data, "data mismatch for {orig_name}");
    }
}

#[test]
fn all_tag_types_round_trip() {
    let doc = cdpmod::CdpDocument {
        version: 1,
        reserved: 0,
        tags: vec![
            CdpTag::Container {
                name: "outer".into(),
                children: vec![
                    CdpTag::Integer {
                        name: "width".into(),
                        values: vec![256],
                    },
                    CdpTag::Integer {
                        name: "coords".into(),
                        values: vec![10, 20, 30],
                    },
                    CdpTag::Float {
                        name: "version".into(),
                        values: vec![3.5],
                    },
                    CdpTag::String {
                        name: "greeting".into(),
                        value: "hello world".into(),
                    },
                    CdpTag::Binary {
                        name: "blob".into(),
                        data: vec![0xFF; 16],
                    },
                    CdpTag::Null {
                        name: "empty".into(),
                    },
                    CdpTag::Kuid {
                        name: "asset-id".into(),
                        user_id: 772861,
                        content_id: 100045,
                    },
                ],
            },
            CdpTag::Null {
                name: "top-level-null".into(),
            },
        ],
    };
    let serialized = cdpmod::serialize(&doc);
    let reparsed = cdpmod::parse(&serialized).unwrap();
    let reserialized = cdpmod::serialize(&reparsed);
    assert_eq!(serialized, reserialized);
}

/// Build a CDP with nested file containers to test structured extraction.
fn build_nested_cdp() -> Vec<u8> {
    let compress = |data: &[u8]| -> Vec<u8> {
        let compressed = lzss::compress(data, 2, 9).unwrap();
        let mut blob = (data.len() as u32).to_le_bytes().to_vec();
        blob.extend_from_slice(&compressed);
        blob
    };

    let doc = cdpmod::CdpDocument {
        version: 1,
        reserved: 0,
        tags: vec![
            CdpTag::Container {
                name: "assets".into(),
                children: vec![CdpTag::Container {
                    name: "<kuid:1:2>".into(),
                    children: vec![
                        CdpTag::Kuid {
                            name: "kuid".into(),
                            user_id: 1,
                            content_id: 2,
                        },
                        CdpTag::String {
                            name: "username".into(),
                            value: "Test Asset".into(),
                        },
                        CdpTag::String {
                            name: "kind".into(),
                            value: "mesh".into(),
                        },
                        CdpTag::Float {
                            name: "trainz-build".into(),
                            values: vec![4.6],
                        },
                        CdpTag::String {
                            name: "compression".into(),
                            value: "LZSS".into(),
                        },
                        CdpTag::Container {
                            name: "kuid-table".into(),
                            children: vec![],
                        },
                        CdpTag::Container {
                            name: "files".into(),
                            children: vec![
                                CdpTag::Container {
                                    name: "art".into(),
                                    children: vec![CdpTag::Binary {
                                        name: "thumb.jpg".into(),
                                        data: compress(b"fake jpeg data"),
                                    }],
                                },
                                CdpTag::Container {
                                    name: "meshes".into(),
                                    children: vec![
                                        CdpTag::Binary {
                                            name: "body.mesh".into(),
                                            data: compress(b"fake mesh data"),
                                        },
                                        CdpTag::Binary {
                                            name: "body.texture.txt".into(),
                                            data: compress(b"texture_path=body.png"),
                                        },
                                    ],
                                },
                                CdpTag::Binary {
                                    name: "readme.txt".into(),
                                    data: compress(b"hello from root of files"),
                                },
                            ],
                        },
                    ],
                }],
            },
            CdpTag::String {
                name: "kind".into(),
                value: "archive".into(),
            },
        ],
    };
    cdpmod::serialize(&doc)
}

#[test]
fn extract_produces_config_json() {
    let cdp = build_nested_cdp();
    let doc = cdpmod::parse(&cdp).unwrap();
    let entries = extract::extract_all(&doc).unwrap();

    let config = entries
        .iter()
        .find(|e| e.path.ends_with("config.json"))
        .expect("no config.json found");
    assert!(config.path.starts_with("assets/<kuid:1:2>/"));

    let json: serde_json::Value = serde_json::from_slice(&config.data).unwrap();
    assert_eq!(json["username"], "Test Asset");
    assert_eq!(json["kind"], "mesh");
    assert_eq!(json["kuid"], "<kuid:1:2>");
    // f32 4.6 → f64 ≈ 4.5999999... due to float widening
    let tb = json["trainz-build"].as_f64().unwrap();
    assert!((tb - 4.6).abs() < 0.001, "trainz-build={tb}");
    assert_eq!(
        json["_generated-by"],
        "https://github.com/emmaworley/cdptool"
    );
    // compression and files tags should be absent
    assert!(json.get("compression").is_none());
    assert!(json.get("files").is_none());
    // kuid-table should be present (empty object)
    assert_eq!(json["kuid-table"], serde_json::json!({}));
}

#[test]
fn extract_preserves_directory_structure() {
    let cdp = build_nested_cdp();
    let doc = cdpmod::parse(&cdp).unwrap();
    let entries = extract::extract_all(&doc).unwrap();

    let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();

    // config.json at asset root
    assert!(paths.contains(&"assets/<kuid:1:2>/config.json"));
    // Nested directories preserved
    assert!(paths.contains(&"assets/<kuid:1:2>/art/thumb.jpg"));
    assert!(paths.contains(&"assets/<kuid:1:2>/meshes/body.mesh"));
    assert!(paths.contains(&"assets/<kuid:1:2>/meshes/body.texture.txt"));
    // Loose file at files root
    assert!(paths.contains(&"assets/<kuid:1:2>/readme.txt"));

    // Verify file contents
    let readme = entries
        .iter()
        .find(|e| e.path.ends_with("readme.txt"))
        .unwrap();
    assert_eq!(readme.data, b"hello from root of files");
}

#[test]
fn extract_all_and_collect_pending_produce_same_paths() {
    let cdp = build_nested_cdp();
    let doc = cdpmod::parse(&cdp).unwrap();

    let eager_paths: Vec<String> = extract::extract_all(&doc)
        .unwrap()
        .iter()
        .map(|e| e.path.clone())
        .collect();

    let lazy_paths: Vec<String> = extract::collect_pending(&doc)
        .unwrap()
        .iter()
        .map(|(p, _)| p.clone())
        .collect();

    assert_eq!(
        eager_paths, lazy_paths,
        "CLI and WASM extraction paths differ"
    );
}

/// A CDP with a tag whose declared length exceeds the actual data.
/// Parsing must not panic.
#[test]
fn malformed_cdp_truncated_tag() {
    // Build a minimal CDP header (16 bytes) + a tag that claims to be 1000 bytes
    // but the file ends after just a few bytes of tag data.
    let mut cdp = Vec::new();
    cdp.extend_from_slice(b"ACS$"); // magic
    cdp.extend_from_slice(&1u32.to_le_bytes()); // version
    cdp.extend_from_slice(&0u32.to_le_bytes()); // reserved
    cdp.extend_from_slice(&100u32.to_le_bytes()); // body length (lie: larger than actual)

    // Tag header: length = 1000 (far past end), name_len=4, "foo\0", type=0x03 (string)
    cdp.extend_from_slice(&1000u32.to_le_bytes());
    cdp.push(4); // name_len (includes null terminator)
    cdp.extend_from_slice(b"foo\0");
    cdp.push(0x03); // tag type: String
    cdp.extend_from_slice(b"hi\0"); // partial payload — far less than claimed 1000 bytes

    // Parsing should succeed (returning whatever tags it can) without panicking.
    let result = cdpmod::parse(&cdp);
    assert!(result.is_ok(), "parse should not fail on truncated tag");
    let doc = result.unwrap();
    // The truncated tag should be skipped entirely (value_end > end).
    assert!(
        doc.tags.is_empty(),
        "truncated tag should be skipped, got {} tags",
        doc.tags.len()
    );
}

/// A CDP with a Binary tag containing fewer than 4 bytes.
/// Extraction must skip it gracefully without panicking.
#[test]
fn malformed_cdp_short_binary() {
    // Build a valid CDP with an asset containing a Binary tag with only 2 bytes of data.
    let short_blob = vec![0xAB, 0xCD]; // only 2 bytes — too short for the 4-byte uncomp_size prefix

    let doc = cdpmod::CdpDocument {
        version: 1,
        reserved: 0,
        tags: vec![CdpTag::Container {
            name: "assets".into(),
            children: vec![CdpTag::Container {
                name: "test-asset".into(),
                children: vec![
                    CdpTag::String {
                        name: "compression".into(),
                        value: "LZSS".into(),
                    },
                    CdpTag::Container {
                        name: "files".into(),
                        children: vec![CdpTag::Binary {
                            name: "short.bin".into(),
                            data: short_blob,
                        }],
                    },
                ],
            }],
        }],
    };

    let cdp_data = cdpmod::serialize(&doc);
    let reparsed = cdpmod::parse(&cdp_data).unwrap();

    // extract_all should skip the short binary (< 4 bytes) and only produce config.json.
    let entries = extract::extract_all(&reparsed).unwrap();
    let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    assert!(
        paths.iter().any(|p| p.ends_with("config.json")),
        "config.json should still be produced"
    );
    assert!(
        !paths.iter().any(|p| p.ends_with("short.bin")),
        "short binary should be skipped, not extracted"
    );
}
