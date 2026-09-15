#!/bin/sh
set -eu
runtime_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
runtime_dependencies=$(cargo tree --locked --manifest-path "$runtime_root/Cargo.toml" -p lns-service --target aarch64-apple-darwin --edges normal --prefix none)
if printf '%s\n' "$runtime_dependencies" | LC_ALL=C awk '$1 ~ /^(egui|eframe|winit|tray-icon)$/ { found=1; print } END { exit !found }'; then
    echo 'FAIL: the macOS service still links a Rust desktop UI' >&2
    exit 1
fi
printf '%s\n' 'PASS: the macOS service has no Rust desktop UI dependencies'
