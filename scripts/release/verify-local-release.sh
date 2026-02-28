#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN_PATH="$ROOT_DIR/target/release/arazzo-cli"

echo "Running private workspace guard..."
bash "$ROOT_DIR/scripts/ci/verify-private-workspace.sh"

echo "Building release binary..."
cargo build --locked --release -p arazzo-cli

echo "Running release binary smoke tests..."
"$BIN_PATH" --json validate "$ROOT_DIR/examples/httpbin-get.arazzo.yaml" >/dev/null
"$BIN_PATH" --json list "$ROOT_DIR/examples/httpbin-get.arazzo.yaml" >/dev/null
"$BIN_PATH" --json run "$ROOT_DIR/examples/httpbin-get.arazzo.yaml" status-check --dry-run --input code=200 >/dev/null

echo "Release binary smoke tests passed."
