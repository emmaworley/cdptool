//! Bit-level I/O wrappers for LSB-first (little-endian) bitstreams.
//!
//! The CDP compression format reads and writes individual bits within bytes
//! starting from the least-significant bit. These wrappers provide a
//! convenient interface on top of the [`bitstream_io`] crate.

use bitstream_io::{BitRead, BitWrite, LittleEndian};

/// Reads bits from a byte slice, least-significant bit first.
///
/// Reading past the end of the data returns `0` rather than panicking,
/// which matches the behavior expected by the adaptive Huffman decoder.
pub struct BitReader<'a> {
    inner: bitstream_io::BitReader<std::io::Cursor<&'a [u8]>, LittleEndian>,
    len_bits: usize,
    pos: usize,
}

impl<'a> BitReader<'a> {
    /// Create a new reader over the given byte slice.
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            inner: bitstream_io::BitReader::new(std::io::Cursor::new(data)),
            len_bits: data.len() * 8,
            pos: 0,
        }
    }

    /// Read a single bit, returning 0 or 1.
    pub fn read_bit(&mut self) -> u32 {
        if self.pos >= self.len_bits {
            return 0;
        }
        self.pos += 1;
        self.inner.read_bit().unwrap_or(false) as u32
    }

    /// Read `n` bits packed into the low bits of a `u32`, LSB first.
    pub fn read_bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let available = (self.len_bits - self.pos).min(n as usize);
        self.pos += available;
        if available == 0 {
            return 0;
        }
        self.inner.read(available as u32).unwrap_or(0)
    }

    /// Current position in the stream, in bits.
    pub fn bit_position(&self) -> usize {
        self.pos
    }
}

/// Writes bits to a growing byte buffer, least-significant bit first.
pub struct BitWriter {
    inner: bitstream_io::BitWriter<Vec<u8>, LittleEndian>,
}

impl Default for BitWriter {
    fn default() -> Self {
        Self {
            inner: bitstream_io::BitWriter::new(Vec::new()),
        }
    }
}

impl BitWriter {
    /// Create a new, empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Write a single bit (only the lowest bit of `bit` is used).
    pub fn write_bit(&mut self, bit: u32) {
        self.inner.write_bit(bit != 0).unwrap();
    }

    /// Write the lowest `n` bits of `value`, LSB first.
    pub fn write_bits(&mut self, value: u32, n: u32) {
        if n > 0 {
            self.inner.write(n, value).unwrap();
        }
    }

    /// Flush any partial byte and return the underlying buffer.
    pub fn finish(mut self) -> Vec<u8> {
        let _ = self.inner.byte_align();
        self.inner.into_writer()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_single_bits() {
        let mut bw = BitWriter::new();
        let bits = [1u32, 0, 1, 1, 0, 0, 1, 0, 1];
        for &b in &bits {
            bw.write_bit(b);
        }
        let data = bw.finish();

        let mut br = BitReader::new(&data);
        for &expected in &bits {
            assert_eq!(br.read_bit(), expected);
        }
    }

    #[test]
    fn round_trip_multi_bits() {
        let mut bw = BitWriter::new();
        bw.write_bits(0b10110, 5);
        bw.write_bits(0xFF, 8);
        bw.write_bits(0b101, 3);
        let data = bw.finish();

        let mut br = BitReader::new(&data);
        assert_eq!(br.read_bits(5), 0b10110);
        assert_eq!(br.read_bits(8), 0xFF);
        assert_eq!(br.read_bits(3), 0b101);
    }

    #[test]
    fn empty_stream() {
        let bw = BitWriter::new();
        let data = bw.finish();
        assert!(data.is_empty());

        let mut br = BitReader::new(&data);
        assert_eq!(br.read_bit(), 0);
    }

    #[test]
    fn byte_packing_order() {
        // 0x29 = 0b0010_1001; writing LSB-first bit by bit should produce this byte.
        let mut bw = BitWriter::new();
        for bit in [1, 0, 0, 1, 0, 1, 0, 0] {
            bw.write_bit(bit);
        }
        assert_eq!(bw.finish(), vec![0x29]);
    }
}
