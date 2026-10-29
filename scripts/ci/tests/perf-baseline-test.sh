#!/usr/bin/env bash
set -euo pipefail

# The fake tools are copies of this file in a disposable repository. They never
# write to the real checkout's target directory or invoke real Cargo.
case "$(basename "$0")" in
  cargo)
    printf 'cargo' >>"$PERF_FAKE_STATE/events"
    printf ' %q' "$@" >>"$PERF_FAKE_STATE/events"
    printf '\n' >>"$PERF_FAKE_STATE/events"
    if [ "$1" = build ]; then
      [ "$PERF_FAKE_MODE" != build ] || exit 23
      mkdir -p "$(dirname "$PERF_FAKE_BINARY")"
      cp "$PERF_HARNESS" "$PERF_FAKE_BINARY"
      chmod +x "$PERF_FAKE_BINARY"
      [ "$PERF_FAKE_MODE" != missing-artifact ] || exit 0
      "$PERF_REAL_PERL" -MJSON::PP -e '
        print encode_json({reason=>"compiler-artifact",target=>{name=>"other",kind=>["bin"]},executable=>"/wrong/other"}), "\n";
        print encode_json({reason=>"compiler-artifact",target=>{name=>"arazzo-cli",kind=>["bin"]},executable=>$ARGV[0]}), "\n";
      ' "$PERF_FAKE_BINARY"
    elif [ "$1" = run ]; then
      # Let the old implementation run safely so the harness proves its gaps.
      shift
      while [ "$1" != -- ]; do shift; done
      shift
      bash "$PERF_HARNESS" --fake-binary "$@"
    else
      exit 88
    fi
    exit 0
    ;;
  perl)
    case "${1:-}" in
      -MTime::HiRes*)
        tick=0
        [ ! -f "$PERF_FAKE_STATE/ticks" ] || read -r tick <"$PERF_FAKE_STATE/ticks"
        tick=$((tick + 1))
        printf '%s\n' "$tick" >"$PERF_FAKE_STATE/ticks"
        printf 'timer %s\n' "$tick" >>"$PERF_FAKE_STATE/events"
        pair=$(((tick - 1) / 2))
        value=$((pair / 20 * 20))
        index=$((pair % 20))
        if [ $((index % 2)) -eq 0 ]; then
          value=$((value + 20 - index / 2))
        else
          value=$((value + (index + 1) / 2))
        fi
        result=$((100000 + pair * 100))
        [ $((tick % 2)) -ne 0 ] || result=$((result + value))
        printf '%s\n' "$result"
        exit 0
        ;;
    esac
    exec "$PERF_REAL_PERL" "$@"
    ;;
  uname)
    [ "$PERF_FAKE_MODE" != host ] || exit 25
    exec "$PERF_REAL_UNAME" "$@"
    ;;
  actual-release-cli) set -- --fake-binary "$@" ;;
  arazzo-cli)
    printf 'WRONG BINARY\n' >>"$PERF_FAKE_STATE/events"
    exit 77
    ;;
esac

if [ "${1:-}" = --fake-binary ]; then
  shift
  if [ "${1:-}" = --version ]; then
    printf 'version\n' >>"$PERF_FAKE_STATE/events"
    [ "$PERF_FAKE_MODE" != version ] || exit 24
    printf 'arazzo-cli fake-1.2.3\n'
    exit 0
  fi
  [ "$1" = --json ] || exit 81
  name="$2"
  case "$name" in
    validate|list) [ "$*" = "--json $name examples/httpbin-get.arazzo.yaml" ] || exit 82 ;;
    run) [ "$*" = '--json run examples/httpbin-get.arazzo.yaml status-check --dry-run --input code=429' ] || exit 83 ;;
    *) exit 84 ;;
  esac
  printf '%s\n' "$0" >>"$PERF_FAKE_STATE/binary-paths"
  count=0
  [ ! -f "$PERF_FAKE_STATE/$name.count" ] || read -r count <"$PERF_FAKE_STATE/$name.count"
  count=$((count + 1))
  printf '%s\n' "$count" >"$PERF_FAKE_STATE/$name.count"
  printf 'binary %s %s' "$name" "$count" >>"$PERF_FAKE_STATE/events"
  printf ' %q' "$@" >>"$PERF_FAKE_STATE/events"
  printf '\n' >>"$PERF_FAKE_STATE/events"
  if [ "$name" = validate ]; then
    [ "$PERF_FAKE_MODE" != warmup ] || [ "$count" -ne 2 ] || exit 7
    [ "$PERF_FAKE_MODE" != measured ] || [ "$count" -ne 5 ] || exit 9
    [ "$PERF_FAKE_MODE" != command ] || exit 11
  fi
  exit 0
fi

TEST_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="${1:-$TEST_DIR/../perf-baseline.sh}"
SCRIPT="$(cd "$(dirname "$SCRIPT")" && pwd)/$(basename "$SCRIPT")"
PERF_HARNESS="$TEST_DIR/$(basename "$0")"
PERF_REAL_PERL="$(command -v perl)"
PERF_REAL_UNAME="$(command -v uname)"
export PERF_HARNESS PERF_REAL_PERL PERF_REAL_UNAME
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/arazzo-perf-test.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT
failures=0

assert_equal() {
  if [ "$1" != "$2" ]; then
    printf 'FAIL: %s (got %s, expected %s)\n' "$3" "$1" "$2" >&2
    failures=$((failures + 1))
  fi
}

assert_contains() {
  if ! grep -F -- "$2" "$1" >/dev/null; then
    printf 'FAIL: %s\n' "$3" >&2
    failures=$((failures + 1))
  fi
}

run_fixture() {
  PERF_FAKE_MODE="$1"
  fixture="$TEMP_ROOT/$1 repo"
  PERF_FAKE_STATE="$fixture/state"
  PERF_FAKE_BINARY="$fixture/custom target/triple/release/actual-release-cli"
  export PERF_FAKE_MODE PERF_FAKE_STATE PERF_FAKE_BINARY
  mkdir -p "$fixture/scripts/ci" "$fixture/tools" "$PERF_FAKE_STATE" "$fixture/target/release" "$fixture/caller location"
  cp "$SCRIPT" "$fixture/scripts/ci/perf-baseline.sh"
  cp "$PERF_HARNESS" "$fixture/tools/cargo"
  cp "$PERF_HARNESS" "$fixture/tools/perl"
  cp "$PERF_HARNESS" "$fixture/tools/uname"
  cp "$PERF_HARNESS" "$fixture/target/release/arazzo-cli"
  chmod +x "$fixture/tools/cargo" "$fixture/tools/perl" "$fixture/tools/uname" "$fixture/target/release/arazzo-cli"
  : >"$PERF_FAKE_STATE/events"
  status=0
  # A relative output path must resolve against the caller, before script cd.
  (cd "$fixture/caller location" && PATH="$fixture/tools:$PATH" bash "$fixture/scripts/ci/perf-baseline.sh" "output with spaces") >"$fixture/run.log" 2>&1 || status=$?
  output="$fixture/caller location/output with spaces"
}

run_fixture success
assert_equal "$status" 0 'successful run status'
assert_equal "$(grep -c '^cargo build ' "$PERF_FAKE_STATE/events" || true)" 1 'one release build'
assert_contains "$PERF_FAKE_STATE/events" 'cargo build --locked --release -p arazzo-cli --message-format=json' 'exact build flags'
assert_equal "$(grep -c '^cargo run ' "$PERF_FAKE_STATE/events" || true)" 0 'Cargo excluded from measured commands'
assert_equal "$(sort -u "$PERF_FAKE_STATE/binary-paths")" "$PERF_FAKE_BINARY" 'every workload uses the selected executable'
assert_equal "$(wc -l <"$PERF_FAKE_STATE/binary-paths" | tr -d ' ')" 69 'sixty-nine total workload invocations'
assert_equal "$(grep -c '^version$' "$PERF_FAKE_STATE/events")" 1 'one separate version provenance invocation'
for name in validate list run; do
  assert_equal "$(cat "$PERF_FAKE_STATE/$name.count")" 23 "$name gets three warmups and twenty samples"
done
assert_equal "$(grep -c '^timer ' "$PERF_FAKE_STATE/events")" 120 'only sixty samples are timed'
assert_equal "$(sed -n '1p' "$PERF_FAKE_STATE/events")" 'cargo build --locked --release -p arazzo-cli --message-format=json' 'build precedes every timer'
assert_equal "$(sed -n '3p' "$PERF_FAKE_STATE/events")" 'binary validate 1 --json validate examples/httpbin-get.arazzo.yaml' 'warmups follow version provenance'
assert_equal "$(sed -n '6p' "$PERF_FAKE_STATE/events")" 'timer 1' 'three warmups precede timing'
assert_contains "$PERF_FAKE_STATE/events" '--json run examples/httpbin-get.arazzo.yaml status-check --dry-run --input code=429' 'original dry-run arguments'
assert_contains "$output/timings.csv" 'name,sample,elapsed_ms,exit_code' 'indexed raw samples'
assert_equal "$(wc -l <"$output/timings.csv" | tr -d ' ')" 61 'sixty raw samples'
assert_contains "$output/summary.md" '| `validate_json` | 20 | 10.5 | 19 |' 'unsorted even median and nearest-rank p95'
assert_contains "$output/summary.md" '| `list_json` | 20 | 30.5 | 39 |' 'second case statistics'
assert_contains "$output/summary.md" '| `run_dry_run_json` | 20 | 50.5 | 59 |' 'third case statistics'
assert_contains "$output/summary.md" "$PERF_FAKE_BINARY" 'actual Cargo artifact path with spaces'
assert_contains "$output/summary.md" 'arazzo-cli fake-1.2.3' 'binary version provenance'
assert_contains "$output/summary.md" 'SHA-256:' 'binary hash provenance'
expected_hash="$("$PERF_REAL_PERL" -MDigest::SHA=sha256_hex -0777 -ne 'print sha256_hex($_)' "$PERF_FAKE_BINARY")"
assert_contains "$output/summary.md" "$expected_hash" 'hash identifies the same selected executable'
assert_contains "$output/summary.md" 'OS/architecture:' 'platform provenance'
assert_contains "$output/summary.md" 'end-to-end process latency' 'measurement semantics'

run_fixture build
assert_equal "$status" 23 'build failure status propagates'
assert_equal "$(grep -c '^timer ' "$PERF_FAKE_STATE/events" || true)" 0 'build failure starts no timer'
assert_contains "$output/summary.md" 'build failed' 'build failure summary retained'

run_fixture missing-artifact
assert_equal "$status" 1 'missing artifact rejects guessed binary'
assert_contains "$output/summary.md" 'artifact selection failed' 'missing artifact evidence'

run_fixture version
assert_equal "$status" 24 'version provenance failure propagates'
assert_contains "$output/summary.md" 'version failed' 'version failure evidence'

run_fixture warmup
assert_equal "$status" 1 'warmup failure returns nonzero'
assert_contains "$output/warmups.csv" 'validate_json,2,7' 'warmup failure retained separately'
assert_equal "$(grep -c '^validate_json,' "$output/timings.csv" || true)" 0 'failed warmup case has no measured samples'
assert_contains "$output/summary.md" '| `validate_json` | 0 | n/a | n/a |' 'no-success case is explicit'

run_fixture host
assert_equal "$status" 1 'host provenance failure returns nonzero'
assert_contains "$output/summary.md" 'OS provenance failed' 'host provenance failure evidence'
assert_equal "$(grep -c '^timer ' "$PERF_FAKE_STATE/events" || true)" 0 'host provenance failure starts no timer'

run_fixture measured
assert_equal "$status" 1 'measured failure returns nonzero'
assert_contains "$output/timings.csv" 'validate_json,2,1,9' 'failed measured sample elapsed time and exit code'
assert_contains "$output/summary.md" '| `validate_json` | 19 | 11 | 20 |' 'statistics exclude failed measured sample'
assert_contains "$output/summary.md" 'FAILED' 'incomplete successful run is labeled'

run_fixture command
assert_equal "$status" 1 'case command failure cannot become success'
assert_contains "$output/warmups.csv" 'validate_json,1,11' 'first command failure evidence'

if [ "$failures" -ne 0 ]; then
  printf '%s assertions failed\n' "$failures" >&2
  exit 1
fi
printf 'Performance baseline harness passed (8 isolated scenarios).\n'
