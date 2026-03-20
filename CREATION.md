# CDP Creation — Future Expansion Plan

The current `cdptool create` command is a minimal proof-of-concept that
wraps files from a flat directory into a CDP archive. A proper creation
tool should reconstruct a CDP from an extracted directory tree, using the
`config.json` files that extraction produces.

## Proposed workflow

```
# 1. Extract a CDP
cdptool extract original.cdp ./work/

# 2. Edit files and metadata
$EDITOR work/assets/<kuid>/config.json
cp new_texture.png work/assets/<kuid>/meshes/

# 3. Repack into a new CDP
cdptool pack work/ modified.cdp
```

## `config.json` as the source of truth

Each `config.json` contains the full tag tree for one asset. The `pack`
command should:

1. Walk the extracted directory tree
2. For each `assets/<name>/config.json`:
   - Parse the JSON back into a CDP tag tree
   - Collect all sibling files and subdirectories as the `files` container
   - Add a `compression "LZSS"` tag
   - LZSS-compress each file
3. Wrap all assets in the top-level CDP structure
4. Serialize and write

### JSON → tag mapping

| JSON type | CDP tag |
|-----------|---------|
| `"string"` | String |
| `123` (integer) | Integer |
| `1.5` (float) | Float |
| `[1, 2, 3]` (number array) | Integer or Float (infer from values) |
| `null` | Null |
| `"<kuid:...>"` or `"<kuid2:...>"` | KUID (parse the string) |
| `{ ... }` (object) | Container |

### Reserved keys

- `_container`: the asset container name (used as the folder name)
- `_generated-by`: provenance URL (ignored on repack)

### Ambiguities to resolve

- **Integer vs Float arrays**: `[1, 2, 3]` could be either. Strategy:
  if any element has a decimal point in the JSON source, use Float;
  otherwise Integer.
- **KUID detection**: strings matching `<kuid:N:N>` or `<kuid2:N:N:N>`
  are KUIDs, not plain strings.
- **Tag ordering**: JSON objects are unordered; CDP tags are ordered.
  Use insertion order from `serde_json::Map` (which preserves parse order)
  or define a canonical sort.

## Directory layout expected by `pack`

```
work/
  assets/
    <kuid2:812750:29289:3>/
      config.json
      art/
        thumbnail.jpg
      meshes/
        body.trainzmesh
        body-color.png
    <kuid2:812750:29290:2>/
      config.json
      script.gs
      art/
        thumbnail.jpg
      sound/
        idling.wav
```

Subdirectories under each asset become nested Container tags inside
`files`. Files without a subdirectory go directly into `files`.

## Top-level CDP metadata

The top-level tags (`kind`, `package-version`, `username`,
`contents-table`, `kuid-table`, `obsolete-table`) should either:

- Be auto-generated with sensible defaults, or
- Be read from a `cdp.json` at the directory root, if present

## Web UI

The web creation UI is deferred. It will need a more complex interface
for editing `config.json` fields, adding/removing files, and managing
multi-asset archives. This is a separate project.
