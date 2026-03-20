.PHONY: test build-cli build-web clean

test:
	cargo test --workspace

build-cli:
	cargo build --release -p cdptool

build-web:
	wasm-pack build crates/cdptool-web --target web --out-dir ../../web/pkg

clean:
	cargo clean
	rm -rf web/pkg
