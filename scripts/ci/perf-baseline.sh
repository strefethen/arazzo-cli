#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_DIR="${1:-${RUNNER_TEMP:-/tmp}/arazzo-perf}"
CSV_FILE="$OUT_DIR/timings.csv"
SUMMARY_FILE="$OUT_DIR/summary.md"

mkdir -p "$OUT_DIR"

now_ms() {
  perl -MTime::HiRes=time -e 'printf("%.0f\n", time() * 1000)'
}

measure() {
  local name="$1"
  shift

  local start_ms
  local end_ms
  local elapsed_ms
  local exit_code

  start_ms="$(now_ms)"
  set +e
  "$@" >/dev/null 2>&1
  exit_code=$?
  set -e
  end_ms="$(now_ms)"
  elapsed_ms=$((end_ms - start_ms))

  echo "${name},${elapsed_ms},${exit_code}" >>"$CSV_FILE"
}

cd "$ROOT_DIR"

echo "name,elapsed_ms,exit_code" >"$CSV_FILE"

measure "validate_json" cargo run -p arazzo-cli -- --json validate examples/httpbin-get.arazzo.yaml
measure "list_json" cargo run -p arazzo-cli -- --json list examples/httpbin-get.arazzo.yaml
measure "run_dry_run_json" cargo run -p arazzo-cli -- --json run examples/httpbin-get.arazzo.yaml status-check --dry-run --input code=429

{
  echo "# Performance Baseline"
  echo
  echo "| Command | Elapsed (ms) | Exit |"
  echo "| --- | ---: | ---: |"
  while IFS=, read -r name elapsed_ms exit_code; do
    if [ "$name" = "name" ]; then
      continue
    fi
    echo "| \`$name\` | $elapsed_ms | $exit_code |"
  done <"$CSV_FILE"
} >"$SUMMARY_FILE"

echo "Perf baseline artifacts written to: $OUT_DIR"
