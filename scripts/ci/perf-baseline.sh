#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_DIR="${1:-${RUNNER_TEMP:-/tmp}/arazzo-perf}"
mkdir -p "$OUT_DIR"
# Resolve caller-relative output paths before changing to the repository.
OUT_DIR="$(cd "$OUT_DIR" && pwd)"
CSV_FILE="$OUT_DIR/timings.csv"
WARMUP_FILE="$OUT_DIR/warmups.csv"
SUMMARY_FILE="$OUT_DIR/summary.md"
WARMUPS=3
SAMPLES=20
mkdir -p "$OUT_DIR/logs"
printf 'name,sample,elapsed_ms,exit_code\n' >"$CSV_FILE"
printf 'name,warmup,exit_code\n' >"$WARMUP_FILE"
printf '# Performance Baseline\n\n' >"$SUMMARY_FILE"

fail() {
  local message="$1"
  local status="${2:-1}"
  printf '\n**FAILED:** %s\n' "$message" >>"$SUMMARY_FILE"
  printf 'Performance baseline: %s\n' "$message" >&2
  exit "$status"
}

now_ms() {
  perl -MTime::HiRes=clock_gettime,CLOCK_MONOTONIC -e 'printf("%.0f\n", clock_gettime(CLOCK_MONOTONIC) * 1000)'
}

record_command() {
  printf '\n```bash\n' >>"$SUMMARY_FILE"
  printf '%q ' "$@" >>"$SUMMARY_FILE"
  printf '\n```\n' >>"$SUMMARY_FILE"
}

measure() {
  local name="$1"
  shift
  local index start_ms end_ms elapsed_ms exit_code failed=0
  printf '\nWorkload: `%s`\n' "$name" >>"$SUMMARY_FILE"
  record_command "$@"

  for ((index = 1; index <= WARMUPS; index++)); do
    exit_code=0
    "$@" >"$OUT_DIR/logs/$name-warmup-$index.stdout" 2>"$OUT_DIR/logs/$name-warmup-$index.stderr" || exit_code=$?
    printf '%s,%s,%s\n' "$name" "$index" "$exit_code" >>"$WARMUP_FILE" || fail 'cannot write warmup evidence'
    if [ "$exit_code" -ne 0 ]; then
      printf '\nWarmup %s failed (exit %s); this workload has no measured samples.\n' "$index" "$exit_code" >>"$SUMMARY_FILE"
      return 1
    fi
  done

  for ((index = 1; index <= SAMPLES; index++)); do
    start_ms="$(now_ms)" || fail 'start clock failed'
    exit_code=0
    "$@" >"$OUT_DIR/logs/$name-sample-$index.stdout" 2>"$OUT_DIR/logs/$name-sample-$index.stderr" || exit_code=$?
    end_ms="$(now_ms)" || fail 'end clock failed'
    [[ "$start_ms" =~ ^[0-9]+$ && "$end_ms" =~ ^[0-9]+$ ]] || fail 'clock returned invalid milliseconds'
    elapsed_ms=$((end_ms - start_ms))
    printf '%s,%s,%s,%s\n' "$name" "$index" "$elapsed_ms" "$exit_code" >>"$CSV_FILE" || fail 'cannot write sample evidence'
    [ "$elapsed_ms" -ge 0 ] || fail 'monotonic clock moved backwards'
    [ "$exit_code" -eq 0 ] || failed=1
  done
  return "$failed"
}

summarize() {
  printf '\n| Command | Successful samples | Median (ms) | p95 (ms) |\n| --- | ---: | ---: | ---: |\n' >>"$SUMMARY_FILE"
  perl -e '
    use strict;
    use warnings;
    my %samples;
    open my $csv, "<", $ARGV[0] or die "$ARGV[0]: $!";
    <$csv>;
    while (<$csv>) {
      chomp;
      my ($name, $index, $elapsed, $status) = split /,/;
      push @{$samples{$name}}, 0 + $elapsed if $status == 0;
    }
    for my $name (qw(validate_json list_json run_dry_run_json)) {
      my @sorted = sort { $a <=> $b } @{$samples{$name} // []};
      my $n = @sorted;
      if (!$n) {
        print "| `$name` | 0 | n/a | n/a |\n";
        next;
      }
      my $median = $n % 2 ? $sorted[int($n/2)] : ($sorted[$n/2-1] + $sorted[$n/2])/2;
      my $p95 = $sorted[int((95*$n + 99)/100)-1];
      printf "| `%s` | %d | %g | %g |\n", $name, $n, $median, $p95;
    }
  ' "$CSV_FILE" >>"$SUMMARY_FILE"
}

cd "$ROOT_DIR"
{
  printf 'Observations are end-to-end process latency in milliseconds, including process startup and measurement/logging overhead.\n\n'
  printf 'Each workload uses %s untimed warmups and %s measured invocations.\n' "$WARMUPS" "$SAMPLES"
  printf 'Only successful measured samples enter statistics. Median sorts samples and averages the two middle values for an even count; nearest-rank p95 is sorted sample ceil(0.95 * n), or the 19th of 20. No successes yields n/a.\n\n'
  printf 'Raw measured outcomes: `timings.csv`; untimed outcomes: `warmups.csv`; command stdout/stderr: `logs/`.\n'
  printf '\nWorking directory: `%s`\n' "$ROOT_DIR"
} >>"$SUMMARY_FILE"
record_command cargo build --locked --release -p arazzo-cli --message-format=json

# Cargo reports the executable actually produced, including configured targets
# and target directories. Never guess target/release or use a stale executable.
build_status=0
cargo build --locked --release -p arazzo-cli --message-format=json >"$OUT_DIR/build.jsonl" 2>"$OUT_DIR/build.stderr" || build_status=$?
[ "$build_status" -eq 0 ] || fail "release build failed (exit $build_status); see build.stderr" "$build_status"
if ! BIN_PATH="$(perl -MJSON::PP -e '
  use strict;
  use warnings;
  my %paths;
  while (<>) {
    my $message = decode_json($_);
    next unless $message->{reason} eq "compiler-artifact";
    next unless $message->{target}{name} eq "arazzo-cli";
    next unless grep { $_ eq "bin" } @{$message->{target}{kind}};
    $paths{$message->{executable}} = 1 if defined $message->{executable};
  }
  die "expected exactly one arazzo-cli executable\n" unless keys(%paths) == 1;
  print((keys %paths)[0]);
' "$OUT_DIR/build.jsonl" 2>"$OUT_DIR/artifact.stderr")"; then
  fail 'artifact selection failed; see artifact.stderr'
fi
[[ "$BIN_PATH" = /* && -x "$BIN_PATH" ]] || fail 'artifact selection failed: executable must be absolute and executable'
printf '\nBinary: `%s`\n' "$BIN_PATH" >>"$SUMMARY_FILE"

if command -v shasum >/dev/null 2>&1; then
  HASH="$(shasum -a 256 "$BIN_PATH")" || fail 'binary SHA-256 failed'
elif command -v sha256sum >/dev/null 2>&1; then
  HASH="$(sha256sum "$BIN_PATH")" || fail 'binary SHA-256 failed'
else
  fail 'binary SHA-256 requires shasum or sha256sum'
fi
HASH="${HASH%% *}"
[[ "$HASH" =~ ^[[:xdigit:]]{64}$ ]] || fail 'binary SHA-256 was invalid'
printf '\nSHA-256: `%s`\n' "$HASH" >>"$SUMMARY_FILE"
version_status=0
"$BIN_PATH" --version >"$OUT_DIR/version.txt" 2>"$OUT_DIR/version.stderr" || version_status=$?
[ "$version_status" -eq 0 ] || fail "binary version failed (exit $version_status); see version.stderr" "$version_status"
VERSION="$(cat "$OUT_DIR/version.txt")" || fail 'cannot read binary version evidence'
[ -n "$VERSION" ] || fail 'binary version failed: empty output'
OS_NAME="$(uname -s)" || fail 'OS provenance failed'
ARCH_NAME="$(uname -m)" || fail 'architecture provenance failed'
[[ -n "$OS_NAME" && -n "$ARCH_NAME" ]] || fail 'OS/architecture provenance was empty'
printf '\nVersion: `%s`\n\nOS/architecture: `%s / %s`\n' "$VERSION" "$OS_NAME" "$ARCH_NAME" >>"$SUMMARY_FILE"

overall=0
measure validate_json "$BIN_PATH" --json validate examples/httpbin-get.arazzo.yaml || overall=1
measure list_json "$BIN_PATH" --json list examples/httpbin-get.arazzo.yaml || overall=1
measure run_dry_run_json "$BIN_PATH" --json run examples/httpbin-get.arazzo.yaml status-check --dry-run --input code=429 || overall=1
summarize
if [ "$overall" -ne 0 ]; then
  fail 'one or more workload invocations failed; statistics exclude failed measured samples'
fi
printf '\nResult: PASS (all warmups and measured invocations succeeded).\n' >>"$SUMMARY_FILE"
printf 'Perf baseline artifacts written to: %s\n' "$OUT_DIR"
