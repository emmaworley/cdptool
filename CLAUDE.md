# cdptool — Development Notes

## Consistency rule

The CLI (`crates/cdptool/src/bin/main.rs`), web UI (`web/app.js` +
`crates/cdptool-web/src/lib.rs`), and README (`README.md`) must stay
in agreement. They do not need strict feature parity — the web UI
intentionally omits some CLI flags — but there must be no
contradictions between them:

- If a CLI subcommand changes behavior, update the README usage section.
- If the compression format changes, update the README spec tables AND
  verify the web UI still round-trips correctly.
- If a new tag type is added to the parser, add it to the README tag
  type table.
- If the web UI's pack/unpack behavior changes (e.g. default mode or
  level), note the difference in the README if it diverges from the CLI
  defaults.
- The CLI `extract` and web UI zip must produce identical directory
  structures and identical `config.json` files. Both use the shared
  `extract` module — do not duplicate extraction logic.

When in doubt, read all three and check for inconsistencies before
finishing a change.

## Build commands

```
make test       # cargo +nightly careful test --workspace
make lint       # cargo +stable clippy ... -D warnings
make fmt        # cargo +stable fmt --check
make build-web  # wasm-pack build → web/pkg/
make build-cli  # cargo build --release -p cdptool
```

## Project layout

```
crates/cdptool/           library + CLI binary (published to crates.io)
  src/extract.rs          shared extraction logic (config.json, directory structure)
crates/cdptool-web/       WASM bindings (not published, thin wrapper over extract)
web/                      static site deployed to GitHub Pages
CREATION.md               plan for future CDP creation from config.json
```

## Warnings policy

`#![deny(warnings)]` and `#![deny(clippy::all)]` are set in
`crates/cdptool/src/lib.rs`. All warnings are compile errors.
