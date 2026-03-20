//! LZSS compression and decompression with adaptive Huffman coding.
//!
//! The CDP format compresses embedded files using an LZSS sliding-window
//! scheme where literals and match lengths are entropy-coded through an
//! adaptive Huffman tree, and match distances use a second independent tree.
//!
//! # Wire format
//!
//! Each compressed blob starts with a single header byte:
//!
//! ```text
//! bits 7-4: mode  (0 = 8-bit distance, 1 = 12-bit, 2 = 14-bit)
//! bits 3-0: level (0 = stored uncompressed, 1–15 = compressed)
//! ```
//!
//! The remaining bytes form an LSB-first bitstream of Huffman-coded symbols:
//!
//! | Symbol    | Meaning |
//! |-----------|---------|
//! | 0–255     | Literal byte |
//! | 256–271   | Match length code (see [`LENGTH_TABLE`]) |
//! | 272       | End of stream |
//!
//! After a length code, extra length bits and a Huffman-coded distance follow.

use crate::bitstream::{BitReader, BitWriter};
use crate::error::CdpError;
use crate::huffman::AdaptiveHuffmanTree;

/// Length code table: `(base_length, extra_bits)` for symbols 256–271.
///
/// The actual match length is `base + read_bits(extra) + 3`.
pub const LENGTH_TABLE: [(u32, u32); 16] = [
    (0, 0),
    (1, 0),
    (2, 0),
    (3, 0),
    (4, 0),
    (5, 1),
    (7, 1),
    (9, 2),
    (13, 2),
    (17, 3),
    (25, 3),
    (33, 4),
    (49, 4),
    (65, 5),
    (97, 5),
    (129, 7),
];

const EOF_SYMBOL: u32 = 272;
const NUM_LIT_SYMBOLS: usize = 273;

/// Return the distance-encoding parameters for a given compression mode.
///
/// Returns `(total_distance_bits, num_distance_symbols, distance_extra_bits)`.
pub fn mode_params(mode: u8) -> Result<(u32, usize, u32), CdpError> {
    match mode {
        0 => Ok((8, 16, 4)),
        1 => Ok((12, 64, 6)),
        2 => Ok((14, 64, 8)),
        _ => Err(CdpError::InvalidMode(mode)),
    }
}

/// Decompress LZSS+Adaptive Huffman compressed data.
///
/// `comp_data` starts with the mode/level header byte, followed by the bitstream.
/// Returns the decompressed bytes.
pub fn decompress(comp_data: &[u8], uncomp_size: usize) -> Result<Vec<u8>, CdpError> {
    if comp_data.is_empty() {
        return Err(CdpError::TruncatedInput);
    }

    let first_byte = comp_data[0];
    let mode = (first_byte >> 4) & 3;
    let level = first_byte & 0xf;

    if level == 0 {
        let end = (1 + uncomp_size).min(comp_data.len());
        return Ok(comp_data[1..end].to_vec());
    }

    let (dist_bits_total, num_dist_symbols, dist_extra_bits) = mode_params(mode)?;
    let dist_mask = (1u32 << dist_bits_total) - 1;

    let mut lit_tree = AdaptiveHuffmanTree::new(NUM_LIT_SYMBOLS);
    let mut dist_tree = AdaptiveHuffmanTree::new(num_dist_symbols);
    let mut br = BitReader::new(&comp_data[1..]);
    let mut output: Vec<u8> = Vec::with_capacity(uncomp_size);

    while output.len() < uncomp_size {
        let sym = lit_tree.decode(&mut br)?;

        if sym == EOF_SYMBOL {
            break;
        }
        if sym > EOF_SYMBOL {
            return Err(CdpError::DecompressFailed(format!("invalid symbol {sym}")));
        }

        if sym < 256 {
            output.push(sym as u8);
        } else {
            let (base, extra) = LENGTH_TABLE[(sym - 256) as usize];
            let extra_val = if extra > 0 { br.read_bits(extra) } else { 0 };
            let match_len = (base + extra_val + 3) as usize;

            let dist_sym = dist_tree.decode(&mut br)?;
            let dist_extra = br.read_bits(dist_extra_bits);
            let distance = ((dist_sym << dist_extra_bits) | dist_extra) & dist_mask;
            let distance = if distance == 0 { 1 } else { distance } as usize;

            for _ in 0..match_len {
                if output.len() >= uncomp_size {
                    break;
                }
                let byte = if distance <= output.len() {
                    output[output.len() - distance]
                } else {
                    0
                };
                output.push(byte);
            }
        }
    }

    output.truncate(uncomp_size);
    Ok(output)
}

// --- Compression ---

/// Max match length: base=129, extra=7 bits (max 127), + 3 = 259.
const MAX_MATCH_LEN: usize = 259;
const MIN_MATCH_LEN: usize = 3;
const HASH_BITS: usize = 13;
const HASH_SIZE: usize = 1 << HASH_BITS;
const MAX_CHAIN: usize = 128;

/// Find the length code, extra bits count, and extra bits value for a given match length.
fn encode_length(match_len: usize) -> (u32, u32, u32) {
    let len_minus_3 = (match_len - 3) as u32;
    let mut code = 0u32;
    for i in (0..16).rev() {
        if LENGTH_TABLE[i].0 <= len_minus_3 {
            code = i as u32;
            break;
        }
    }
    let (base, extra_bits) = LENGTH_TABLE[code as usize];
    (code, extra_bits, len_minus_3 - base)
}

/// Split a distance into (distance_symbol, extra_bits_value).
fn encode_distance(distance: usize, dist_extra_bits: u32) -> (u32, u32) {
    let d = distance as u32;
    let dist_sym = d >> dist_extra_bits;
    let extra = d & ((1u32 << dist_extra_bits) - 1);
    (dist_sym, extra)
}

/// Hash-chain match finder for LZSS compression.
struct MatchFinder {
    head: Vec<u32>,
    prev: Vec<u32>,
    window_mask: usize,
}

impl MatchFinder {
    fn new(window_size: usize) -> Self {
        Self {
            head: vec![u32::MAX; HASH_SIZE],
            prev: vec![u32::MAX; window_size],
            window_mask: window_size - 1,
        }
    }

    fn hash3(data: &[u8], pos: usize) -> usize {
        if pos + 2 >= data.len() {
            return 0;
        }
        let h = (data[pos] as usize)
            ^ ((data[pos + 1] as usize) << 4)
            ^ ((data[pos + 2] as usize) << 8);
        h & (HASH_SIZE - 1)
    }

    fn insert(&mut self, data: &[u8], pos: usize) {
        if pos + 2 >= data.len() {
            return;
        }
        let h = Self::hash3(data, pos);
        self.prev[pos & self.window_mask] = self.head[h];
        self.head[h] = pos as u32;
    }

    fn find_best(&self, data: &[u8], pos: usize, max_dist: usize) -> Option<(usize, usize)> {
        if pos + 2 >= data.len() {
            return None;
        }
        let h = Self::hash3(data, pos);
        let mut match_pos = self.head[h];
        let mut best_len = MIN_MATCH_LEN - 1;
        let mut best_dist = 0;
        let max_len = MAX_MATCH_LEN.min(data.len() - pos);
        let mut chain_len = 0;

        while match_pos != u32::MAX && chain_len < MAX_CHAIN {
            let mp = match_pos as usize;
            let dist = pos - mp;
            if dist == 0 || dist > max_dist {
                match_pos = self.prev[mp & self.window_mask];
                chain_len += 1;
                continue;
            }

            // Compare bytes
            let mut len = 0;
            while len < max_len && data[mp + len] == data[pos + len] {
                len += 1;
            }
            if len > best_len {
                best_len = len;
                best_dist = dist;
                if len == max_len {
                    break;
                }
            }

            match_pos = self.prev[mp & self.window_mask];
            chain_len += 1;
        }

        if best_len >= MIN_MATCH_LEN {
            Some((best_dist, best_len))
        } else {
            None
        }
    }
}

/// Compress data using LZSS + Adaptive Huffman coding.
///
/// Returns compressed bytes starting with the mode/level header byte.
pub fn compress(data: &[u8], mode: u8, level: u8) -> Result<Vec<u8>, CdpError> {
    if data.is_empty() {
        let mut out = vec![(mode << 4) | level];
        if level == 0 {
            return Ok(out);
        }
        // For empty data with level > 0, just write EOF
        let mut lit_tree = AdaptiveHuffmanTree::new(NUM_LIT_SYMBOLS);
        let mut bw = BitWriter::new();
        lit_tree.encode(&mut bw, EOF_SYMBOL);
        out.extend_from_slice(&bw.finish());
        return Ok(out);
    }

    if level == 0 {
        let mut out = vec![mode << 4];
        out.extend_from_slice(data);
        return Ok(out);
    }

    let (dist_bits_total, num_dist_symbols, dist_extra_bits) = mode_params(mode)?;
    let max_dist = ((1u32 << dist_bits_total) - 1) as usize;
    let window_size = max_dist.next_power_of_two();

    let mut lit_tree = AdaptiveHuffmanTree::new(NUM_LIT_SYMBOLS);
    let mut dist_tree = AdaptiveHuffmanTree::new(num_dist_symbols);
    let mut bw = BitWriter::new();
    let mut mf = MatchFinder::new(window_size);

    let mut pos = 0;
    while pos < data.len() {
        let best = mf.find_best(data, pos, max_dist);
        mf.insert(data, pos);

        if let Some((distance, length)) = best {
            let (len_code, extra_bits, extra_val) = encode_length(length);
            lit_tree.encode(&mut bw, 256 + len_code);
            if extra_bits > 0 {
                bw.write_bits(extra_val, extra_bits);
            }

            let (dist_sym, dist_extra) = encode_distance(distance, dist_extra_bits);
            dist_tree.encode(&mut bw, dist_sym);
            bw.write_bits(dist_extra, dist_extra_bits);

            // Insert skipped positions into hash chains
            for i in 1..length {
                mf.insert(data, pos + i);
            }
            pos += length;
        } else {
            lit_tree.encode(&mut bw, data[pos] as u32);
            pos += 1;
        }
    }

    lit_tree.encode(&mut bw, EOF_SYMBOL);

    let mut out = vec![(mode << 4) | level];
    out.extend_from_slice(&bw.finish());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_hello() {
        let data = b"hello world hello hello world";
        let compressed = compress(data, 2, 9).unwrap();
        let decompressed = decompress(&compressed, data.len()).unwrap();
        assert_eq!(&decompressed, data);
    }

    #[test]
    fn round_trip_empty() {
        let data = b"";
        let compressed = compress(data, 2, 9).unwrap();
        let decompressed = decompress(&compressed, 0).unwrap();
        assert_eq!(&decompressed, data);
    }

    #[test]
    fn round_trip_single_byte() {
        let data = b"\x42";
        let compressed = compress(data, 2, 9).unwrap();
        let decompressed = decompress(&compressed, 1).unwrap();
        assert_eq!(&decompressed, data);
    }

    #[test]
    fn round_trip_all_zeros() {
        let data = vec![0u8; 4096];
        let compressed = compress(&data, 2, 9).unwrap();
        assert!(compressed.len() < data.len() / 2, "should compress well");
        let decompressed = decompress(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn round_trip_incompressible() {
        // Pseudorandom data: won't compress much but must round-trip
        let data: Vec<u8> = (0..1000).map(|i| ((i * 137 + 43) % 256) as u8).collect();
        let compressed = compress(&data, 2, 9).unwrap();
        let decompressed = decompress(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn round_trip_across_rebuild() {
        // ~40KB of data forces at least one Huffman tree rebuild
        let data: Vec<u8> = (0..40_000).map(|i| ((i * 97 + 31) % 256) as u8).collect();
        let compressed = compress(&data, 2, 9).unwrap();
        let decompressed = decompress(&compressed, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn round_trip_store_mode() {
        let data = b"stored uncompressed";
        let compressed = compress(data, 2, 0).unwrap();
        assert_eq!(compressed[0], 0x20); // mode=2, level=0
        let decompressed = decompress(&compressed, data.len()).unwrap();
        assert_eq!(&decompressed, data);
    }

    #[test]
    fn encode_length_table() {
        // Verify encode_length is inverse of decode
        assert_eq!(encode_length(3), (0, 0, 0)); // min match
        assert_eq!(encode_length(4), (1, 0, 0));
        assert_eq!(encode_length(8), (5, 1, 0)); // base=5, extra=1 bit, val=0 → 5+0+3=8
        assert_eq!(encode_length(9), (5, 1, 1)); // base=5, extra=1 bit, val=1 → 5+1+3=9
    }
}
