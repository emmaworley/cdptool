use cdptool::lzss;
use std::fs;
use std::path::Path;

fn fixture(name: &str) -> Vec<u8> {
    fs::read(Path::new("tests/fixtures").join(name)).unwrap()
}

fn assert_round_trip(data: &[u8], label: &str) {
    let compressed = lzss::compress(data, 2, 9).unwrap();
    let decompressed = lzss::decompress(&compressed, data.len()).unwrap();
    assert_eq!(decompressed.len(), data.len(), "{label}: length mismatch");
    assert_eq!(&decompressed, data, "{label}: data mismatch");
}

// --- Adversarial fixtures ---

#[test]
fn round_trip_empty() {
    assert_round_trip(&fixture("empty.bin"), "empty");
}

#[test]
fn round_trip_one_byte() {
    assert_round_trip(&fixture("one_byte.bin"), "one_byte");
}

#[test]
fn round_trip_zeros() {
    assert_round_trip(&fixture("zeros_1k.bin"), "zeros_1k");
}

#[test]
fn round_trip_ff() {
    assert_round_trip(&fixture("ff_1k.bin"), "ff_1k");
}

#[test]
fn round_trip_pattern() {
    assert_round_trip(&fixture("pattern_8k.bin"), "pattern_8k");
}

#[test]
fn round_trip_near_rebuild() {
    assert_round_trip(&fixture("near_rebuild.bin"), "near_rebuild");
}

#[test]
fn round_trip_over_rebuild() {
    assert_round_trip(&fixture("over_rebuild.bin"), "over_rebuild");
}

#[test]
fn round_trip_large() {
    assert_round_trip(&fixture("large.bin"), "large");
}

// --- Synthetic data tests ---

#[test]
fn round_trip_all_modes() {
    let data: Vec<u8> = (0..8192).map(|i| ((i * 37 + 13) % 256) as u8).collect();
    for mode in 0..=2u8 {
        let compressed = lzss::compress(&data, mode, 9).unwrap();
        let decompressed = lzss::decompress(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data, "mode {mode} round-trip failed");
    }
}

#[test]
fn round_trip_multiple_rebuilds() {
    // 200 KB of data will force several Huffman tree rebuilds.
    let data: Vec<u8> = (0..200_000).map(|i| ((i * 97 + 31) % 256) as u8).collect();
    let compressed = lzss::compress(&data, 2, 9).unwrap();
    let decompressed = lzss::decompress(&compressed, data.len()).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn round_trip_highly_compressible() {
    // Single repeated byte: should compress extremely well.
    let data = vec![0x42u8; 100_000];
    let compressed = lzss::compress(&data, 2, 9).unwrap();
    assert!(
        compressed.len() < data.len() / 10,
        "expected >10x compression, got {} -> {}",
        data.len(),
        compressed.len()
    );
    let decompressed = lzss::decompress(&compressed, data.len()).unwrap();
    assert_eq!(decompressed, data);
}

#[test]
fn store_mode_is_identity() {
    let data = b"no compression at all";
    let compressed = lzss::compress(data, 2, 0).unwrap();
    // Header byte + raw data
    assert_eq!(compressed.len(), 1 + data.len());
    assert_eq!(compressed[0] & 0x0F, 0, "level should be 0");
    let decompressed = lzss::decompress(&compressed, data.len()).unwrap();
    assert_eq!(&decompressed, data);
}

// --- Mode-specific decompression tests ---
//
// The bitstream LZSS decompressor (level 1–6) has no encoder, so we can't
// round-trip through it directly. However, we can verify that data compressed
// at level 9 (Huffman) and then stored in each mode decompresses correctly
// when the mode parameter is varied during compression + decompression.

/// Verify compress(mode=0, level=9) → decompress round-trips correctly.
#[test]
fn round_trip_mode_0() {
    let data: Vec<u8> = (0..4096).map(|i| ((i * 37 + 13) % 256) as u8).collect();
    let compressed = lzss::compress(&data, 0, 9).unwrap();
    let decompressed = lzss::decompress(&compressed, data.len()).unwrap();
    assert_eq!(
        decompressed, data,
        "mode 0 (8-bit distance) round-trip failed"
    );
}

/// Verify compress(mode=1, level=9) → decompress round-trips correctly.
#[test]
fn round_trip_mode_1() {
    let data: Vec<u8> = (0..4096).map(|i| ((i * 37 + 13) % 256) as u8).collect();
    let compressed = lzss::compress(&data, 1, 9).unwrap();
    let decompressed = lzss::decompress(&compressed, data.len()).unwrap();
    assert_eq!(
        decompressed, data,
        "mode 1 (12-bit distance) round-trip failed"
    );
}
