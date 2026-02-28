#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echo "Verifying private workspace publish settings..."

manifests=("$ROOT_DIR/Cargo.toml" "$ROOT_DIR"/crates/*/Cargo.toml)

if grep -nE '^publish[[:space:]]*=[[:space:]]*true[[:space:]]*$' "${manifests[@]}"; then
  echo
  echo "ERROR: Found publish=true in one or more manifests."
  echo "This repository is private-only and must not publish externally."
  exit 1
fi

if ! grep -qE '^publish[[:space:]]*=[[:space:]]*false[[:space:]]*$' "$ROOT_DIR/Cargo.toml"; then
  echo "ERROR: Root workspace Cargo.toml must set publish = false."
  exit 1
fi

missing=0
for manifest in "$ROOT_DIR"/crates/*/Cargo.toml; do
  if ! grep -qE '^publish[[:space:]]*=[[:space:]]*false[[:space:]]*$' "$manifest"; then
    echo "ERROR: Missing explicit publish = false in $manifest"
    missing=1
  fi
done

if [ "$missing" -ne 0 ]; then
  echo
  echo "ERROR: All workspace crates must explicitly set publish = false."
  exit 1
fi

echo "Private publish settings verified."
