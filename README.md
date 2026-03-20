# cdptool

Pack and unpack CDP (CHUMP) archive files.

CDP is a binary container format used to distribute game assets.
Each archive holds a tree of typed tags — metadata like names and
identifiers alongside LZSS-compressed file payloads.

**Web UI:** <https://emma.gg/cdptool>
— drop a `.cdp` file to extract its contents as a zip. No install required.

## Install

```
cargo install cdptool
```

## Usage

### Extract files from an archive

```
cdptool extract archive.cdp out/
```

This produces a directory tree mirroring the CDP's internal structure:

```
out/assets/<kuid2:812750:29289:3>/
  config.json           ← all asset metadata as JSON
  art/thumbnail.jpg
  meshes/body.trainzmesh
  meshes/body-color.png
```

Each asset gets a `config.json` containing the full tag tree (all
metadata except the `files` container and `compression` tag). This
format is designed for future round-tripping — see [CREATION.md](CREATION.md).

### Inspect archive structure

```
cdptool info archive.cdp
```

### Create an archive (experimental)

> **Note:** `create` is a minimal proof-of-concept that wraps files from
> a flat directory. It does not read `config.json` or preserve nested
> directory structure. See [CREATION.md](CREATION.md) for the planned
> full creation workflow.

```
cdptool create new.cdp my_files/
cdptool create new.cdp my_files/ --username "Alice"
cdptool create new.cdp my_files/ --level 0   # store without compression
```

| Flag | Default | Description |
|------|---------|-------------|
| `--username <name>` | `unknown` | Creator name stored in the archive |
| `--level <0–15>` | `9` | Compression level (`0` = store verbatim, `9` = best) |

---

## CDP File Format Specification

### File header

Every CDP file begins with a 16-byte header:

| Offset | Size | Type | Description |
|--------|------|------|-------------|
| 0 | 4 | `[u8; 4]` | Magic: `ACS$` (0x41 0x43 0x53 0x24) |
| 4 | 4 | `u32 LE` | Format version |
| 8 | 4 | `u32 LE` | Reserved (always 0) |
| 12 | 4 | `u32 LE` | Body length (bytes following this header) |

### Tag structure

The body is a sequence of tags. Each tag:

```
┌──────────┬──────────┬──────────────────┬──────────┬─────────┐
│ tag_len  │ name_len │ name + NUL       │ type     │ payload │
│ 4 bytes  │ 1 byte   │ name_len bytes   │ 1 byte   │ varies  │
│ u32 LE   │          │ (omitted if 0)   │          │         │
└──────────┴──────────┴──────────────────┴──────────┴─────────┘
```

`tag_len` counts everything after itself: `1 + name_len + 1 + len(payload)`.

`name_len` includes the trailing NUL. A value of 0 means no name.

### Tag types

| Byte | Name | Payload |
|------|------|---------|
| `0x00` | Container | Zero or more nested tags |
| `0x01` | Integer | One or more `i32 LE` values |
| `0x02` | Float | One or more `f32 LE` (IEEE 754) values |
| `0x03` | String | NUL-terminated UTF-8 |
| `0x04` | Binary | Raw bytes |
| `0x05` | Null | Empty (0 bytes) |
| `0x0D` | KUID | 8 bytes: `u32 LE` user\_id + `i32 LE` content\_id |

**KUID bit layout:**

```
user_id bits 0–21:  uid     (22-bit unsigned)
user_id bits 22–23: flags   (2-bit)
content_id:         signed 32-bit identifier
```

### Binary tag payload (compressed files)

Binary tags that hold file data are prefixed with a 4-byte uncompressed
size, followed by the LZSS-compressed stream:

```
┌──────────────┬────────────────────────┐
│ uncomp_size  │ compressed stream      │
│ 4 bytes      │ variable               │
│ u32 LE       │                        │
└──────────────┴────────────────────────┘
```

---

## LZSS Compression

### Stream header byte

The first byte of the compressed stream encodes two fields:

```
bits 7–4: mode   (0, 1, or 2)
bits 3–0: level  (0 = stored, 1–6 = bitstream LZSS, 7–15 = Huffman LZSS)
```

| Level | Encoding |
|-------|----------|
| 0 | Stored verbatim (no compression) |
| 1–6 | Bitstream LZSS: flag words + inline tokens |
| 7–15 | Adaptive Huffman LZSS: entropy-coded tokens |

### Compression modes

The mode selects the distance encoding width. All three levels share
the same mode table:

| Mode | Header nibble | Total distance bits | Distance tree symbols | Distance extra bits |
|------|---------------|---------------------|-----------------------|---------------------|
| 0 | `0x0_` | 8 | 16 | 4 |
| 1 | `0x1_` | 12 | 64 | 6 |
| 2 | `0x2_` | 14 | 64 | 8 |

Distance = `(dist_symbol << extra_bits) | read_bits(extra_bits)`, masked
to `total_distance_bits`. A decoded distance of 0 is treated as 1.

### Bitstream LZSS (levels 1–6)

Tokens are packed into 32-bit flag words read LSB-first. Each flag word
is followed by the payload bytes for its tokens. The flag bits determine
the token type.

#### Mode 0 (8-bit distance)

Each token consumes 1 or 4 flag bits:

| Flag bits | Token | Payload |
|-----------|-------|---------|
| `0` | Literal | 1 byte |
| `1` + 3 length bits | Back-reference | 1-byte distance |

The 3 length bits encode match length − 3 (range 0–6). If all three
bits are set (value 7), an escape byte follows in the payload giving
the actual length − 3 (range 7–255).

#### Modes 1 & 2 (12/14-bit distance)

Each token consumes 1 or 2 flag bits:

| Flag bits | Token | Payload |
|-----------|-------|---------|
| `00` | Literal | 1 byte |
| `01` | Short run | 1 length byte, then that many + 3 raw bytes |
| `1` | Back-reference | 16-bit word: distance \| (length << shift) |

The 16-bit word packs distance in the low bits and length − 3 in the
high bits. If the length field is maxed out (all bits set), an escape
byte follows with the actual length − 3.

### Huffman LZSS (levels 7–15)

Literals, length codes, and distance symbols are entropy-coded through
adaptive Huffman trees. The bitstream is a sequence of Huffman-coded
symbols:

| Symbol | Meaning |
|--------|---------|
| 0–255 | Literal byte |
| 256–271 | Match length code (index into length table) |
| 272 | End of stream |

A back-reference is encoded as:

1. Huffman-encode `256 + length_code` through the literal/length tree
2. Write extra length bits (if any) raw into the bitstream
3. Huffman-encode the distance symbol through the distance tree
4. Write distance extra bits raw into the bitstream

### Length table

Shared by both bitstream and Huffman levels. Each length code maps to a
base value and a count of extra bits. Match length = `base + extra + 3`.

| Code | Base | Extra bits | Length range |
|------|------|------------|-------------|
| 0 | 0 | 0 | 3 |
| 1 | 1 | 0 | 4 |
| 2 | 2 | 0 | 5 |
| 3 | 3 | 0 | 6 |
| 4 | 4 | 0 | 7 |
| 5 | 5 | 1 | 8–9 |
| 6 | 7 | 1 | 10–11 |
| 7 | 9 | 2 | 12–15 |
| 8 | 13 | 2 | 16–19 |
| 9 | 17 | 3 | 20–27 |
| 10 | 25 | 3 | 28–35 |
| 11 | 33 | 4 | 36–51 |
| 12 | 49 | 4 | 52–67 |
| 13 | 65 | 5 | 68–99 |
| 14 | 97 | 5 | 100–131 |
| 15 | 129 | 7 | 132–259 |

Note: bitstream levels use the length table only for Huffman levels.
At bitstream levels, match length − 3 is encoded directly in flag bits
or escape bytes as described above.

### Adaptive Huffman tree

Both encoder and decoder maintain two adaptive Huffman trees — one for
literals/lengths (273 symbols: 0–272) and one for distances (symbol
count depends on mode). The trees stay synchronized because both sides
call the same update procedure after each symbol.

**Tree layout.** Nodes are stored in a flat array sorted by
non-decreasing frequency. Leaf nodes store `symbol | 0x8000` in the
child array. Internal nodes store the index of their left child; the
right child is implicitly at `left_child + 1`.

| Property | Literal/length tree | Distance tree (mode 2) |
|----------|--------------------|-----------------------|
| Symbols | 273 (0–272) | 64 |
| Leaves | 274 (rounded even) | 64 |
| Total nodes | 547 | 127 |
| Root index | 546 | 126 |

**Update.** After encoding or decoding a symbol, frequencies are
incremented along the leaf-to-root path. When a node's frequency
exceeds its right neighbor's, it is swapped rightward to restore sorted
order — the Gallager–Knuth–Vitter sibling property.

**Rebuild.** When the root frequency exceeds `0x7FFF` (32 767), all
leaf frequencies are halved (`(freq + 1) >> 1`) and internal nodes are
reconstructed. The rebuild uses two strategies to maintain sorted order:

- **Shift-right**: scan backward for the insertion point, shift nodes
  right with `memmove`, update child→parent pointers.
- **Leaf promotion**: move lower-frequency leaves earlier in the array
  to make room for the new internal node.

---

## License

MIT
