//! WASM bindings for cdptool — streaming CDP extraction.
//!
//! Exposes a stateful [`CdpExtractor`] that parses a CDP once and yields
//! one decompressed file at a time, plus [`info_cdp`] for displaying the
//! tag tree. The JS side feeds extracted files into a streaming zip writer.

use cdptool::cdp as cdpmod;
use cdptool::extract;
use cdptool::lzss;
use wasm_bindgen::prelude::*;

fn err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Stateful extractor. Construct with CDP bytes, then call `next_file()` repeatedly.
#[wasm_bindgen]
pub struct CdpExtractor {
    /// `(zip_path, compressed_blob)` pairs from [`extract::collect_pending`].
    pending: Vec<(String, Vec<u8>)>,
    cursor: usize,
}

#[wasm_bindgen]
impl CdpExtractor {
    /// Parse a CDP and prepare for streaming extraction.
    #[wasm_bindgen(constructor)]
    pub fn new(cdp_bytes: &[u8]) -> Result<CdpExtractor, JsValue> {
        let doc = cdpmod::parse(cdp_bytes).map_err(err)?;
        let pending = extract::collect_pending(&doc).map_err(err)?;
        Ok(CdpExtractor { pending, cursor: 0 })
    }

    /// Total number of entries (config.json files + real files).
    pub fn total(&self) -> usize {
        self.pending.len()
    }

    /// How many entries have been yielded so far.
    pub fn progress(&self) -> usize {
        self.cursor
    }

    /// Get the next file. Returns null when done.
    ///
    /// Returns a JS array `[path: string, data: Uint8Array]`.
    pub fn next_file(&mut self) -> Result<JsValue, JsValue> {
        if self.cursor >= self.pending.len() {
            return Ok(JsValue::NULL);
        }

        let (path, blob) = &self.pending[self.cursor];
        self.cursor += 1;

        let uncomp_size = u32::from_le_bytes(blob[0..4].try_into().unwrap()) as usize;
        let decompressed = lzss::decompress(&blob[4..], uncomp_size).map_err(err)?;

        let pair = js_sys::Array::new();
        pair.push(&JsValue::from_str(path));
        pair.push(&js_sys::Uint8Array::from(decompressed.as_slice()).into());
        Ok(pair.into())
    }
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
