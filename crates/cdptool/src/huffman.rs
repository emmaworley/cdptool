//! Adaptive Huffman coding for the CDP compression format.
//!
//! This module implements a Gallager–Knuth–Vitter-style adaptive Huffman tree
//! that is shared by both the encoder and decoder. The tree structure uses the
//! *sibling property*: all nodes are stored in a flat array ordered by
//! non-decreasing frequency, and every internal node's two children occupy
//! adjacent positions in the array.
//!
//! # Encoding
//!
//! Leaf nodes have bit 15 set in their `child` entry (i.e. `symbol | 0x8000`).
//! Internal nodes store the index of their left child; the right child is
//! implicitly `left + 1`.
//!
//! After each symbol is encoded or decoded, the tree is updated: frequencies
//! are incremented along the leaf-to-root path, and nodes are swapped to
//! maintain the sorted-frequency invariant. When the root frequency exceeds
//! `0x7FFF` the tree is rebuilt by halving all leaf frequencies.

use crate::bitstream::{BitReader, BitWriter};
use crate::error::CdpError;

const LEAF_FLAG: u32 = 0x8000;
const REBUILD_THRESHOLD: u32 = 0x7FFF;
/// Sentinel marking a node whose parent has not yet been assigned during rebuild.
const UNPROCESSED: u32 = 0xFFFF_FFFE;

/// An adaptive Huffman tree used for both encoding and decoding.
///
/// The encoder and decoder each maintain their own instance; because both call
/// [`update`](Self::update) with the same symbol sequence, their internal
/// states remain synchronized.
pub struct AdaptiveHuffmanTree {
    num_leaves: usize,
    total_nodes: usize,
    root_idx: usize,
    /// Per-node frequency counts, kept in non-decreasing order.
    freq: Vec<u32>,
    /// `parent[i]` = index of node `i`'s parent (root points to itself).
    parent: Vec<u32>,
    /// For leaves: `symbol | 0x8000`. For internal nodes: index of left child.
    child: Vec<u32>,
    /// Reverse map: `sym_to_node[symbol]` = position of that symbol's leaf.
    sym_to_node: Vec<u32>,
}

impl AdaptiveHuffmanTree {
    /// Create a new tree with room for `num_symbols_raw` symbols.
    ///
    /// The actual leaf count is rounded up to the nearest even number. All
    /// leaves start with frequency 1 and internal nodes are built bottom-up
    /// by sequential pairing.
    pub fn new(num_symbols_raw: usize) -> Self {
        let n = (num_symbols_raw + 1) & 0xFFE;
        let total = n * 2 - 1;
        let root = total - 1;
        let mut t = Self {
            num_leaves: n,
            total_nodes: total,
            root_idx: root,
            freq: vec![0; total],
            parent: vec![0; total],
            child: vec![0; total],
            sym_to_node: vec![0; n],
        };
        for i in 0..n {
            t.freq[i] = 1;
            t.child[i] = (i as u32) | LEAF_FLAG;
            t.sym_to_node[i] = i as u32;
        }
        let mut j = 0usize;
        for k in n..total {
            t.freq[k] = t.freq[j] + t.freq[j + 1];
            t.child[k] = j as u32;
            t.parent[j] = k as u32;
            t.parent[j + 1] = k as u32;
            j += 2;
        }
        t.parent[root] = root as u32;
        t
    }

    /// Decode one symbol by reading bits from `br`.
    ///
    /// Walks from the root down to a leaf, reading one bit at each internal
    /// node (0 = left child, 1 = right child). Returns the decoded symbol
    /// and updates the tree.
    pub fn decode(&mut self, br: &mut BitReader) -> Result<u32, CdpError> {
        let mut v = self.child[self.root_idx];
        let mut depth = 0u32;
        while v & LEAF_FLAG == 0 {
            let idx = (v + br.read_bit()) as usize;
            if idx >= self.total_nodes {
                return Err(CdpError::DecompressFailed(
                    "tree index out of bounds".into(),
                ));
            }
            v = self.child[idx];
            depth += 1;
            if depth > 50 {
                return Err(CdpError::DecompressFailed(
                    "tree decode depth exceeded".into(),
                ));
            }
        }
        let sym = v & !LEAF_FLAG;
        self.update(sym);
        Ok(sym)
    }

    /// Encode `symbol` by writing its Huffman code to `bw`.
    ///
    /// Walks from the symbol's leaf up to the root collecting the branch
    /// direction at each level, then writes those bits in reverse (root-to-leaf)
    /// order so the decoder can traverse top-down.
    pub fn encode(&mut self, bw: &mut BitWriter, symbol: u32) {
        let mut bits = Vec::new();
        let mut i = self.sym_to_node[symbol as usize] as usize;
        while i != self.root_idx {
            let p = self.parent[i] as usize;
            let left_child = self.child[p] as usize;
            bits.push(if i == left_child { 0u32 } else { 1u32 });
            i = p;
        }
        for &bit in bits.iter().rev() {
            bw.write_bit(bit);
        }
        self.update(symbol);
    }

    /// Increment frequencies along the leaf-to-root path, swapping nodes as
    /// needed to maintain the sorted-frequency invariant.
    fn update(&mut self, symbol: u32) {
        if self.freq[self.root_idx] > REBUILD_THRESHOLD {
            self.rebuild();
        }

        let mut i = self.sym_to_node[symbol as usize] as usize;

        loop {
            self.freq[i] += 1;
            let f = self.freq[i];

            // If the sort order is violated, find the rightmost node with a
            // smaller frequency and swap with it.
            if i + 1 < self.total_nodes && self.freq[i + 1] < f {
                let mut j = i + 2;
                while j < self.total_nodes && self.freq[j] < f {
                    j += 1;
                }
                j -= 1;

                let ci = self.child[i];
                let cj = self.child[j];

                // Point cj's children/symbol at position i (where cj is moving).
                if cj & LEAF_FLAG == 0 {
                    self.parent[cj as usize] = i as u32;
                    self.parent[cj as usize + 1] = i as u32;
                } else {
                    self.sym_to_node[(cj & !LEAF_FLAG) as usize] = i as u32;
                }

                // Point ci's children/symbol at position j (where ci is moving).
                if ci & LEAF_FLAG == 0 {
                    self.parent[ci as usize] = j as u32;
                    self.parent[ci as usize + 1] = j as u32;
                } else {
                    self.sym_to_node[(ci & !LEAF_FLAG) as usize] = j as u32;
                }

                self.freq.swap(i, j);
                self.freq[j] = f;
                self.child.swap(i, j);

                i = j;
            }

            let p = self.parent[i] as usize;
            if p == i || p == self.root_idx {
                break;
            }
            i = p;
        }

        self.freq[self.root_idx] += 1;
    }

    /// Halve all leaf frequencies and reconstruct the internal nodes.
    ///
    /// The rebuild algorithm preserves the sorted-frequency array layout using
    /// two strategies when inserting newly created internal nodes:
    ///
    /// 1. **Shift-right (memmove)**: when the new node's frequency is smaller
    ///    than preceding nodes, shift them right and insert at the correct
    ///    sorted position.
    /// 2. **Leaf promotion**: when unprocessed leaves with smaller frequencies
    ///    exist after the insertion point, move them earlier in the array to
    ///    make room.
    fn rebuild(&mut self) {
        let root_idx = self.root_idx;
        let mut remaining_leaves = self.num_leaves as i32;

        // Phase 1: halve leaf frequencies and mark all nodes as unprocessed.
        for i in 0..root_idx {
            if self.child[i] & LEAF_FLAG != 0 {
                self.freq[i] = (self.freq[i] + 1) >> 1;
            }
            self.parent[i] = UNPROCESSED;
        }
        self.parent[root_idx] = UNPROCESSED;

        // Phase 2: reconstruct internal nodes by pairing unprocessed children.
        let mut pair_scan = 0u32;
        let mut node_scan = 0u32;

        loop {
            // Find the next unprocessed node (will become a left child).
            let mut found = false;
            while (pair_scan as usize) < root_idx {
                if self.parent[pair_scan as usize] == UNPROCESSED {
                    found = true;
                    break;
                }
                pair_scan += 1;
            }
            if !found || pair_scan as usize >= root_idx {
                self.parent[root_idx] = 0xFFFF_FFFF;
                return;
            }

            let left = pair_scan;
            let right = pair_scan + 1;
            pair_scan += 2;

            // Advance node_scan past leaves (counting them) until we find an
            // unprocessed internal node slot for the new parent.
            while node_scan as usize <= root_idx {
                if self.child[node_scan as usize] & LEAF_FLAG != 0 {
                    remaining_leaves -= 1;
                } else if self.parent[node_scan as usize] == UNPROCESSED {
                    break;
                }
                node_scan += 1;
            }

            let slot = node_scan as usize;
            node_scan += 1;
            let next_scan = node_scan;

            let new_freq = self.freq[left as usize] + self.freq[right as usize];
            let mut insert_at = slot;

            if slot > 0 && self.freq[slot - 1] > new_freq {
                // --- Shift-right path ---
                // Scan backward to find the correct sorted position, updating
                // child→parent pointers for every node we shift.
                let mut scan = (slot - 1) as i32;
                while scan >= 0 && self.freq[scan as usize] > new_freq {
                    let c = self.child[scan as usize];
                    if c & LEAF_FLAG == 0 {
                        self.parent[c as usize] += 1;
                        self.parent[c as usize + 1] += 1;
                    } else {
                        self.sym_to_node[(c & !LEAF_FLAG) as usize] += 1;
                    }
                    scan -= 1;
                }
                let pos = (scan + 1) as usize;
                let count = slot - pos;
                if count > 0 {
                    self.freq.copy_within(pos..pos + count, pos + 1);
                    self.child.copy_within(pos..pos + count, pos + 1);
                }
                insert_at = pos;
            } else if remaining_leaves > 0 {
                // --- Leaf-promotion path ---
                // Move leaves that are smaller than new_freq into earlier slots.
                let mut scan_pos = next_scan as usize;
                let mut dst = slot;
                let mut nxt = next_scan;

                loop {
                    while scan_pos < root_idx {
                        if self.child[scan_pos] & LEAF_FLAG != 0 {
                            break;
                        }
                        scan_pos += 1;
                    }
                    if scan_pos >= root_idx || self.freq[scan_pos] >= new_freq {
                        break;
                    }
                    self.freq[dst] = self.freq[scan_pos];
                    self.child[dst] = self.child[scan_pos];
                    self.parent[dst] = UNPROCESSED;
                    self.sym_to_node[(self.child[scan_pos] & !LEAF_FLAG) as usize] = dst as u32;

                    dst = nxt as usize;
                    self.child[scan_pos] = 0;
                    nxt += 1;
                    node_scan = nxt;

                    remaining_leaves -= 1;
                    if remaining_leaves <= 0 {
                        break;
                    }
                    scan_pos += 1;
                }
                insert_at = dst;
            }

            self.freq[insert_at] = new_freq;
            self.child[insert_at] = left;
            self.parent[left as usize] = insert_at as u32;
            self.parent[right as usize] = insert_at as u32;
        }
    }

    /// Validate the tree's structural invariants (test-only).
    ///
    /// Returns `None` if valid, or a description of the first violation found.
    #[cfg(test)]
    pub fn verify(&self) -> Option<String> {
        for i in 0..self.total_nodes - 1 {
            if self.freq[i] > self.freq[i + 1] {
                return Some(format!(
                    "freq[{i}]={} > freq[{}]={}",
                    self.freq[i],
                    i + 1,
                    self.freq[i + 1],
                ));
            }
        }
        for s in 0..self.num_leaves {
            let pos = self.sym_to_node[s] as usize;
            if pos >= self.total_nodes {
                return Some(format!("sym_to_node[{s}]={pos} out of range"));
            }
            let c = self.child[pos];
            if c & LEAF_FLAG == 0 || (c & !LEAF_FLAG) != s as u32 {
                return Some(format!("sym_to_node[{s}]={pos} but child[{pos}]=0x{c:x}"));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitstream::{BitReader, BitWriter};

    #[test]
    fn encode_decode_round_trip() {
        let symbols: Vec<u32> = (0..256)
            .chain([0, 1, 2, 0, 0, 1, 255, 128].iter().copied())
            .collect();
        let mut enc = AdaptiveHuffmanTree::new(273);
        let mut bw = BitWriter::new();
        for &s in &symbols {
            enc.encode(&mut bw, s);
        }
        let data = bw.finish();

        let mut dec = AdaptiveHuffmanTree::new(273);
        let mut br = BitReader::new(&data);
        for &expected in &symbols {
            assert_eq!(dec.decode(&mut br).unwrap(), expected);
        }
    }

    #[test]
    fn encode_decode_across_rebuild() {
        let mut enc = AdaptiveHuffmanTree::new(273);
        let mut bw = BitWriter::new();
        let mut symbols = Vec::new();
        for i in 0..35_000u32 {
            let s = i % 256;
            symbols.push(s);
            enc.encode(&mut bw, s);
        }
        assert!(enc.verify().is_none(), "encoder tree invalid after rebuild");

        let data = bw.finish();
        let mut dec = AdaptiveHuffmanTree::new(273);
        let mut br = BitReader::new(&data);
        for (idx, &expected) in symbols.iter().enumerate() {
            assert_eq!(dec.decode(&mut br).unwrap(), expected, "mismatch at {idx}");
        }
    }
}
