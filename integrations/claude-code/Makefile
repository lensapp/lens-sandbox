BIN := bin/lns-cc

.PHONY: build test lint fmt clean

build:
	cargo build --release
	@mkdir -p bin
	cp target/release/lns-cc $(BIN)

test:
	cargo test

lint:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

clean:
	cargo clean
	rm -rf bin
