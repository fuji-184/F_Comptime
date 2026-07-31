#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

crate=fcomptime_test
manifest="$crate/Cargo.toml"

echo "==> step 1: regenerate comptime outputs (runs source!/output! as tests)"
cargo clean --manifest-path "$manifest" -p fcomptime_test
rm -rf "$crate/comptime"
cargo test --manifest-path "$manifest" --features=comptime -- --test-threads=1

echo "==> step 2: build & run the e2e binary (asserts call!/func!/info/get!/partial/full/nested)"
cargo run --manifest-path "$manifest" --bin fcomptime_test

echo "==> step 3: run the lib verify binary (cross-target access to comptime outputs)"
cargo run --manifest-path "$manifest" --bin verify

echo "e2e tests: all good"
