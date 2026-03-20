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

/// Decompress LZSS-compressed data.
///
/// `comp_data` starts with the mode/level header byte, followed by the bitstream.
/// Dispatches to stored (level 0), bitstream LZSS (level 1–6), or
/// adaptive Huffman LZSS (level 7–15) based on the header.
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

    if level < 7 {
        decompress_bitstream(
            &comp_data[1..],
            uncomp_size,
            dist_bits_total,
            dist_extra_bits,
        )
    } else {
        decompress_huffman(
            &comp_data[1..],
            uncomp_size,
            dist_bits_total,
            num_dist_symbols,
            dist_extra_bits,
        )
    }
}

/// Level 7–15: Adaptive Huffman coded LZSS.
fn decompress_huffman(
    stream: &[u8],
    uncomp_size: usize,
    dist_bits_total: u32,
    num_dist_symbols: usize,
    dist_extra_bits: u32,
) -> Result<Vec<u8>, CdpError> {
    let dist_mask = (1u32 << dist_bits_total) - 1;
    let mut lit_tree = AdaptiveHuffmanTree::new(NUM_LIT_SYMBOLS);
    let mut dist_tree = AdaptiveHuffmanTree::new(num_dist_symbols);
    let mut br = BitReader::new(stream);
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

            copy_match(&mut output, distance, match_len, uncomp_size);
        }
    }

    output.truncate(uncomp_size);
    Ok(output)
}

/// Level 1–6: Bitstream LZSS without Huffman coding.
///
/// The stream is a sequence of 32-bit flag words. Each bit in a flag word
/// indicates the type of the next token. The flag word is consumed from
/// LSB to MSB (bit 0 first, then bit 1, etc up to bit 31).
///
/// For modes with `dist_bits_total > 8` (modes 1 and 2):
///   - Two consecutive flag bits select the token type:
///     - `0,0`: literal byte (read 1 byte)
///     - `0,1`: short run (read 1 length byte, then copy `length+3` bytes from stream)
///     - `1,_`: back-reference (read 2-byte distance/length word)
///
/// For mode 0 (`dist_bits_total == 8`):
///   - One flag bit selects the type:
///     - `0`: literal byte
///     - `1`: back-reference with 3 inline length bits + 1-byte distance
fn decompress_bitstream(
    stream: &[u8],
    uncomp_size: usize,
    dist_bits_total: u32,
    dist_extra_bits: u32,
) -> Result<Vec<u8>, CdpError> {
    let dist_mask = (1u32 << dist_bits_total) - 1;
    let len_overflow = if dist_bits_total < 16 {
        (1u32 << (16 - dist_bits_total)) - 1
    } else {
        0
    };
    let offset_shift = dist_bits_total.min(15);

    let mut br = BitReader::new(stream);
    let mut output: Vec<u8> = Vec::with_capacity(uncomp_size);

    if dist_extra_bits == 0 {
        // Mode 0: simple 1-bit flags, 8-bit distance, inline 3-bit length
        while output.len() < uncomp_size {
            let flags = br.read_bits(32);
            let mut mask = 1u32;

            while mask != 0 && output.len() < uncomp_size {
                if flags & mask == 0 {
                    // Literal byte
                    output.push(br.read_bits(8) as u8);
                    mask <<= 1;
                } else {
                    // Back-reference: read 3 length bits from the flag stream
                    mask <<= 1;
                    if mask == 0 {
                        break;
                    }

                    let mut len_code = if flags & mask != 0 { 1u32 } else { 0 };
                    mask <<= 1;
                    if mask == 0 {
                        break;
                    }
                    len_code |= if flags & mask != 0 { 2 } else { 0 };
                    mask <<= 1;
                    if mask == 0 {
                        break;
                    }
                    len_code |= if flags & mask != 0 { 4 } else { 0 };

                    // Escape: if all 3 bits set, read a full byte for length
                    let len_code = if len_code == 7 {
                        br.read_bits(8)
                    } else {
                        len_code
                    };

                    let distance = br.read_bits(8) as usize;
                    let distance = if distance == 0 { 1 } else { distance };
                    let match_len = (len_code + 3) as usize;

                    copy_match(&mut output, distance, match_len, uncomp_size);
                    mask <<= 1;
                }
            }
        }
    } else {
        // Modes 1 & 2: 2-bit flag pairs, 16-bit distance/length words
        while output.len() < uncomp_size {
            let flags = br.read_bits(32);
            let mut mask = 1u32;

            while mask != 0 && output.len() < uncomp_size {
                if flags & mask == 0 {
                    // First bit is 0: check second bit
                    mask <<= 1;
                    if mask == 0 {
                        break;
                    }

                    if flags & mask == 0 {
                        // 0,0 = literal byte
                        output.push(br.read_bits(8) as u8);
                    } else {
                        // 0,1 = short copy: read length byte, copy that many bytes from stream
                        let run_len = br.read_bits(8) as usize + 3;
                        for _ in 0..run_len {
                            if output.len() >= uncomp_size {
                                break;
                            }
                            output.push(br.read_bits(8) as u8);
                        }
                    }
                    mask <<= 1;
                } else {
                    // First bit is 1: back-reference with 16-bit word
                    let word = br.read_bits(16);
                    let distance = (word & dist_mask) as usize;
                    let mut len_code = (word >> offset_shift) & len_overflow;

                    // Escape: if length field is maxed out, read a full byte
                    if len_code == len_overflow {
                        len_code = br.read_bits(8);
                    }

                    let distance = if distance == 0 { 1 } else { distance };
                    let match_len = (len_code + 3) as usize;

                    copy_match(&mut output, distance, match_len, uncomp_size);
                    mask <<= 1;
                }
            }
        }
    }

    output.truncate(uncomp_size);
    Ok(output)
}

/// Copy `match_len` bytes from `distance` positions back in the output buffer.
fn copy_match(output: &mut Vec<u8>, distance: usize, match_len: usize, max_size: usize) {
    for _ in 0..match_len {
        if output.len() >= max_size {
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

/// Compress data using LZSS.
///
/// Level 0 stores data verbatim. Levels 1–6 use bitstream LZSS (flag words).
/// Levels 7–15 use adaptive Huffman coded LZSS.
///
/// Returns compressed bytes starting with the mode/level header byte.
pub fn compress(data: &[u8], mode: u8, level: u8) -> Result<Vec<u8>, CdpError> {
    if level == 0 || data.is_empty() {
        let mut out = vec![(mode << 4) | level];
        if level == 0 {
            out.extend_from_slice(data);
            return Ok(out);
        }
        // Empty data with level > 0: just write Huffman EOF
        let mut lit_tree = AdaptiveHuffmanTree::new(NUM_LIT_SYMBOLS);
        let mut bw = BitWriter::new();
        lit_tree.encode(&mut bw, EOF_SYMBOL);
        out.extend_from_slice(&bw.finish());
        return Ok(out);
    }

    let (dist_bits_total, _, dist_extra_bits) = mode_params(mode)?;

    if level < 7 {
        compress_bitstream(data, mode, level, dist_bits_total, dist_extra_bits)
    } else {
        compress_huffman(data, mode, level, dist_bits_total, dist_extra_bits)
    }
}

/// Level 7–15: Adaptive Huffman coded LZSS.
fn compress_huffman(
    data: &[u8],
    mode: u8,
    level: u8,
    dist_bits_total: u32,
    dist_extra_bits: u32,
) -> Result<Vec<u8>, CdpError> {
    let (_, num_dist_symbols, _) = mode_params(mode)?;
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

/// A token produced by the match finder, before encoding into the flag-word format.
enum Token {
    Literal(u8),
    Match { distance: usize, length: usize },
}

/// Level 1–6: Bitstream LZSS with flag words.
///
/// Tokens are grouped into blocks of up to 32 flag bits. The flag word is
/// written first, then the payload bytes for each token in that block.
fn compress_bitstream(
    data: &[u8],
    mode: u8,
    level: u8,
    dist_bits_total: u32,
    dist_extra_bits: u32,
) -> Result<Vec<u8>, CdpError> {
    let max_dist = ((1u32 << dist_bits_total) - 1) as usize;
    let window_size = max_dist.next_power_of_two();
    let dist_mask = (1u32 << dist_bits_total) - 1;
    let offset_shift = dist_bits_total.min(15);
    let len_overflow = if dist_bits_total < 16 {
        (1u32 << (16 - dist_bits_total)) - 1
    } else {
        0
    };

    // Max match length for bitstream modes: mode 0 can encode 3..258 (escape byte),
    // modes 1&2 can encode 3..258 (escape byte).
    let max_match = 258usize;

    let mut mf = MatchFinder::new(window_size);
    let mut tokens = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        let best = mf.find_best(data, pos, max_dist);
        mf.insert(data, pos);

        if let Some((distance, length)) = best {
            let length = length.min(max_match);
            for i in 1..length {
                mf.insert(data, pos + i);
            }
            tokens.push(Token::Match { distance, length });
            pos += length;
        } else {
            tokens.push(Token::Literal(data[pos]));
            pos += 1;
        }
    }

    // Encode tokens into flag-word blocks
    let mut bw = BitWriter::new();
    let mut ti = 0;

    if dist_extra_bits == 0 {
        // Mode 0: 1-bit flags, 3-bit inline length, 8-bit distance
        while ti < tokens.len() {
            let mut flags = 0u32;
            let mut payload = BitWriter::new();
            let mut bit = 0u32;

            while bit < 32 && ti < tokens.len() {
                match &tokens[ti] {
                    Token::Literal(b) => {
                        // flag bit = 0 (literal)
                        payload.write_bits(*b as u32, 8);
                        bit += 1;
                        ti += 1;
                    }
                    Token::Match { distance, length } => {
                        // Need 1 + 3 = 4 flag bits for a match
                        if bit + 4 > 32 {
                            break;
                        }
                        flags |= 1 << bit; // flag bit = 1 (match)
                        bit += 1;

                        let len_code = (*length as u32) - 3;
                        if len_code >= 7 {
                            // All 3 bits set + escape byte
                            flags |= 7 << bit;
                            payload.write_bits(len_code, 8);
                        } else {
                            flags |= len_code << bit;
                        }
                        bit += 3;

                        payload.write_bits(*distance as u32, 8);
                        ti += 1;
                    }
                }
            }

            bw.write_bits(flags, 32);
            let payload_bytes = payload.finish();
            for &b in &payload_bytes {
                bw.write_bits(b as u32, 8);
            }
        }
    } else {
        // Modes 1 & 2: 2-bit flag pairs, 16-bit distance/length words
        while ti < tokens.len() {
            let mut flags = 0u32;
            let mut payload = BitWriter::new();
            let mut bit = 0u32;

            while bit < 32 && ti < tokens.len() {
                match &tokens[ti] {
                    Token::Literal(b) => {
                        // Need 2 flag bits: 0,0
                        if bit + 2 > 32 {
                            break;
                        }
                        // flags bits are already 0,0
                        payload.write_bits(*b as u32, 8);
                        bit += 2;
                        ti += 1;
                    }
                    Token::Match { distance, length } => {
                        // Need 1 flag bit: 1
                        if bit + 1 > 32 {
                            break;
                        }
                        flags |= 1 << bit;
                        bit += 1;

                        let d = (*distance as u32) & dist_mask;
                        let len_code = (*length as u32) - 3;

                        if len_code >= len_overflow {
                            // Escape: max out the length field, then write full byte
                            let word = d | (len_overflow << offset_shift);
                            payload.write_bits(word, 16);
                            payload.write_bits(len_code, 8);
                        } else {
                            let word = d | (len_code << offset_shift);
                            payload.write_bits(word, 16);
                        }
                        ti += 1;
                    }
                }
            }

            bw.write_bits(flags, 32);
            let payload_bytes = payload.finish();
            for &b in &payload_bytes {
                bw.write_bits(b as u32, 8);
            }
        }
    }

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
        assert_eq!(encode_length(3), (0, 0, 0));
        assert_eq!(encode_length(4), (1, 0, 0));
        assert_eq!(encode_length(8), (5, 1, 0));
        assert_eq!(encode_length(9), (5, 1, 1));
    }

    // --- Bitstream LZSS (level 1-6) round-trip tests ---

    fn assert_bitstream_round_trip(data: &[u8], mode: u8, level: u8) {
        let compressed = compress(data, mode, level).unwrap();
        assert_eq!(compressed[0], (mode << 4) | level);
        let decompressed = decompress(&compressed, data.len()).unwrap();
        assert_eq!(
            decompressed,
            data,
            "mode={mode} level={level} len={}",
            data.len()
        );
    }

    #[test]
    fn bitstream_mode0_level1_hello() {
        assert_bitstream_round_trip(b"hello world hello hello world", 0, 1);
    }

    #[test]
    fn bitstream_mode0_level6_repeated() {
        assert_bitstream_round_trip(&vec![0xAA; 1024], 0, 6);
    }

    #[test]
    fn bitstream_mode0_single_byte() {
        assert_bitstream_round_trip(b"\x42", 0, 1);
    }

    #[test]
    fn bitstream_mode1_level3_mixed() {
        let data: Vec<u8> = (0..2000).map(|i| ((i * 37 + 13) % 256) as u8).collect();
        assert_bitstream_round_trip(&data, 1, 3);
    }

    #[test]
    fn bitstream_mode2_level6_compressible() {
        let data = b"abcabcabcabcabcabc".repeat(100);
        assert_bitstream_round_trip(&data, 2, 6);
    }

    #[test]
    fn bitstream_mode2_level1_incompressible() {
        let data: Vec<u8> = (0..500).map(|i| ((i * 137 + 43) % 256) as u8).collect();
        assert_bitstream_round_trip(&data, 2, 1);
    }

    #[test]
    fn bitstream_all_modes_all_levels() {
        let data: Vec<u8> = (0..4096).map(|i| ((i * 97 + 31) % 256) as u8).collect();
        for mode in 0..=2u8 {
            for level in 1..=6u8 {
                assert_bitstream_round_trip(&data, mode, level);
            }
        }
    }

    #[test]
    fn bitstream_mode0_long_match_escape() {
        // 300 zeros → should produce a match longer than 7, triggering the escape byte
        assert_bitstream_round_trip(&vec![0u8; 300], 0, 1);
    }

    #[test]
    fn bitstream_mode2_long_match_escape() {
        // Should produce matches exceeding len_overflow, triggering escape
        assert_bitstream_round_trip(&vec![0u8; 500], 2, 1);
    }
}
