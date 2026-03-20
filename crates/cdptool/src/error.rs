//! Error types for CDP archive operations.

use thiserror::Error;

/// Errors that can occur during CDP parsing, compression, or decompression.
#[derive(Debug, Error)]
pub enum CdpError {
    /// An underlying I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The file does not start with the `ACS$` magic bytes.
    #[error("not a CDP file (bad magic)")]
    InvalidMagic,

    /// Encountered an unrecognized tag type byte.
    #[error("unknown tag type 0x{0:02x}")]
    InvalidTagType(u8),

    /// The input ended before a complete structure could be read.
    #[error("truncated input")]
    TruncatedInput,

    /// The LZSS+Huffman decompressor encountered corrupt data.
    #[error("decompression failed: {0}")]
    DecompressFailed(String),

    /// The compression mode byte is outside the valid range (0–2).
    #[error("invalid compression mode {0}")]
    InvalidMode(u8),
}
