#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

die_missing() {
  local command_name="$1"
  local setup_hint="$2"

  echo "ERROR: required command '$command_name' is not available." >&2
  echo "Install/setup: $setup_hint" >&2
  exit 127
}

require_command() {
  local command_name="$1"
  local setup_hint="$2"

  if ! command -v "$command_name" >/dev/null 2>&1; then
    die_missing "$command_name" "$setup_hint"
  fi
}

require_cargo_subcommand() {
  local subcommand="$1"
  local setup_hint="$2"

  if ! cargo "$subcommand" --version >/dev/null 2>&1; then
    die_missing "cargo $subcommand" "$setup_hint"
  fi
}

run_step() {
  local label="$1"
  shift

  printf '\n==> %s\n' "$label"
  "$@"
}

run_step_in() {
  local label="$1"
  local directory="$2"
  shift 2

  printf '\n==> %s\n' "$label"
  (cd "$directory" && "$@")
}

cd "$ROOT_DIR"

require_command bash "Use an environment with bash available."
require_command cargo "Install Rust with rustup and use the repository toolchain."

run_step "Private workspace guard" bash "$ROOT_DIR/scripts/ci/verify-private-workspace.sh"

require_cargo_subcommand fmt "Install rustfmt for the active toolchain: rustup component add rustfmt"
run_step "Rust format check" cargo fmt --all -- --check

require_cargo_subcommand clippy "Install Clippy for the active toolchain: rustup component add clippy"
run_step "Rust clippy" cargo clippy --workspace --all-targets --all-features -- -D warnings

run_step "Rust workspace tests" cargo test --workspace
run_step "Schema drift test" cargo test -p arazzo-cli --test schema_drift

require_command npm "Install Node.js/npm, then run npm install from vscode-arazzo-debug."
run_step_in "VS Code extension typecheck (npm run lint)" "$ROOT_DIR/vscode-arazzo-debug" npm run lint
run_step_in "VS Code extension build (npm run build)" "$ROOT_DIR/vscode-arazzo-debug" npm run build

run_step "Release binary smoke checks" bash "$ROOT_DIR/scripts/release/verify-local-release.sh"

require_cargo_subcommand audit "Install cargo-audit: cargo install cargo-audit --locked"
run_step "RustSec advisory audit (cargo audit)" cargo audit

printf '\nRelease readiness checks passed.\n'
