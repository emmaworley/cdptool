.PHONY: test lint fmt build-cli build-web clean

test:
	cargo +nightly careful test --workspace

lint:
	cargo +stable clippy --workspace --all-targets -- -D warnings

fmt:
	cargo +stable fmt --check

build-cli:
	cargo build --release -p cdptool

build-web:
	wasm-pack build crates/cdptool-web --target web --out-dir ../../web/pkg

clean:
	cargo clean
	rm -rf web/pkg
