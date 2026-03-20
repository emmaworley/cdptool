use cdptool::cdp as cdpmod;
use cdptool::cdp::CdpTag;
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
