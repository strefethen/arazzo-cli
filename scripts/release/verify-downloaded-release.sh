#!/usr/bin/env bash
set -euo pipefail

if [ "${1:-}" = "" ]; then
  echo "Usage: bash scripts/release/verify-downloaded-release.sh <tag> [output_dir]" >&2
  echo "Example: bash scripts/release/verify-downloaded-release.sh v0.1.0" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TAG="$1"
OUT_DIR="${2:-$ROOT_DIR/tmp/release-$TAG}"

mkdir -p "$OUT_DIR"

echo "Checking release exists for tag $TAG..."
gh release view "$TAG" >/dev/null

echo "Downloading release assets to $OUT_DIR..."
gh release download "$TAG" --dir "$OUT_DIR" --clobber

if [ ! -f "$OUT_DIR/SHA256SUMS.txt" ]; then
  echo "ERROR: SHA256SUMS.txt was not found in $OUT_DIR" >&2
  exit 1
fi

echo "Verifying checksums..."
(
  cd "$OUT_DIR"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c SHA256SUMS.txt
  else
    shasum -a 256 -c SHA256SUMS.txt
  fi
)

os_name="$(uname -s)"
local_asset=""

case "$os_name" in
  Darwin)
    local_asset="arazzo-cli-${TAG}-macos"
    ;;
  Linux)
    local_asset="arazzo-cli-${TAG}-linux-x86_64"
    ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    local_asset="arazzo-cli-${TAG}-windows-x86_64.exe"
    ;;
esac

if [ "$local_asset" = "" ]; then
  echo "Skipping smoke test on unsupported host OS: $os_name"
  exit 0
fi

binary_path="$OUT_DIR/$local_asset"
if [ ! -f "$binary_path" ]; then
  echo "Skipping smoke test; no local-host binary asset found at $binary_path"
  exit 0
fi

if [ "$os_name" != "Windows_NT" ] && [ ! -x "$binary_path" ]; then
  chmod +x "$binary_path"
fi

echo "Running smoke test with downloaded binary..."
"$binary_path" --json validate "$ROOT_DIR/examples/httpbin-get.arazzo.yaml" >/dev/null

echo "Downloaded release validation passed for $TAG."
