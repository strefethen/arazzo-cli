# Root parse and validation benchmark

`root_load` measures prepared YAML bytes through exactly one public
`arazzo_validate::parse_bytes_with_diagnostics` call per iteration. It has
10-, 100- and 1,000-step series, each labeled with serialized byte count.
Criterion 0.5.1 owns sampling and statistics. There is no performance budget.

All six committed fixtures are read before sampling. The fixed OpenAPI source
must match an independently constructed complete YAML value and resolve every
operation. All positive roots must match independently constructed complete
models with zero diagnostics. Malformed YAML must produce `ParseYaml`; the
duplicate-step control must produce exactly its expected validation diagnostic.
Every preflight completes before registration of any measured series.

Each `iter_custom` callback sums separate monotonic intervals around the public
call. Black-boxing bytes precedes the timer. Checking the result, destroying it,
and updating counts follow the timer. The next call cannot retain the previous
model. Internal parser allocations/destruction and timer overhead are included.
Reads, fixture construction, hashing, source checks, result destruction and
oracle checks are excluded. There is no second validation call. Criterion also
observes callback wall time for scheduling; the returned summed intervals are
its measured values.

`stderr` records preflight/control outcomes and requested/successful/failed
iteration accounting per series, including warmup and smoke iterations. These
totals include native Criterion callbacks, not just sampled iterations. Native
`sample.json` iteration counts describe sampled measurements. Any first oracle
failure exits nonzero with case and count context. Treat the entire invocation
as invalid even if earlier series already wrote output. Missing series, missing
final `root_load PASS invocation`, or a nonzero exit also invalidate the run.

## Build and identify the executable

Run from the repository root on the actual measurement host, after focused
tests and independent review of the candidate. Keep the checkout and fixtures
unchanged from provenance collection through both runs. Use an idle machine
with the same power configuration for repeats. The following is a macOS/zsh
example using installed Cargo, Python 3 and macOS host tools; it adds no runner
or Rust hashing dependency. Each evidence directory is new and ignored by Git.

```sh
set -e
mkdir -p target
evidence="$(mktemp -d "$PWD/target/root-load-evidence.XXXXXX")"
printf '%s\n' "$evidence"
printf '%s\n' 'cargo bench -p arazzo-validate --bench root_load --no-run --message-format=json' > "$evidence/build.command"
if cargo bench -p arazzo-validate --bench root_load --no-run --message-format=json > "$evidence/build.jsonl" 2> "$evidence/build.stderr"; then
  build_status=0
else
  build_status=$?
fi
printf '%s\n' "$build_status" > "$evidence/build.exit"
test "$build_status" -eq 0
python3 -c 'import json,pathlib,sys; rows=[json.loads(s) for s in pathlib.Path(sys.argv[1]).read_text().splitlines() if s.strip()]; paths={r["executable"] for r in rows if r.get("reason")=="compiler-artifact" and r.get("target",{}).get("name")=="root_load" and "bench" in r.get("target",{}).get("kind",[]) and r.get("executable")}; assert len(paths)==1, paths; p=pathlib.Path(paths.pop()).resolve(); assert p.is_file(),p; print(p)' "$evidence/build.jsonl" > "$evidence/executable.path"
binary="$(cat "$evidence/executable.path")"
```

Stop if any step fails. Do not guess a hashed filename or choose an older binary
from `target/release/deps`. Cargo's JSON is the build/executable record. Keep
build output separate from process measurements. `cargo bench` uses the bench
profile, inheriting release optimization with this workspace's bench debuginfo.
The command enables default features only, with no explicit target override.
The benchmark uses Rust's default system allocator; it adds no allocator.
This package-scoped Criterion 0.5.1 build enables clap's `std` feature without
its `help` feature. Use the supported `--test` smoke invocation below to verify
the discovered executable; `--help` exits with an argument error.

## Record provenance before measuring

```sh
fixtures="$PWD/crates/arazzo-validate/benches/fixtures/root_load"
git rev-parse HEAD > "$evidence/source.commit"
git status --porcelain=v1 --untracked-files=all > "$evidence/source.status"
git diff --binary HEAD -- Cargo.toml Cargo.lock rust-toolchain.toml crates/arazzo-validate > "$evidence/source.diff"
rustc -Vv > "$evidence/rustc.version"
cargo -V > "$evidence/cargo.version"
python3 --version > "$evidence/python.version" 2>&1
shasum --version > "$evidence/shasum.version"
sw_vers > "$evidence/os.version"
uname -a > "$evidence/kernel"
system_profiler SPHardwareDataType > "$evidence/hardware"
sysctl -n hw.memsize > "$evidence/ram.bytes"
pmset -g batt > "$evidence/power.before"
uptime > "$evidence/load.before"
ps -Ao pid,pcpu,pmem,comm > "$evidence/processes.before"
shasum -a 256 "$binary" Cargo.lock "$fixtures/steps_10.yaml" "$fixtures/steps_100.yaml" "$fixtures/steps_1000.yaml" "$fixtures/openapi.yaml" "$fixtures/malformed.yaml" "$fixtures/duplicate_step.yaml" > "$evidence/inputs.sha256"
wc -c "$binary" Cargo.lock "$fixtures/steps_10.yaml" "$fixtures/steps_100.yaml" "$fixtures/steps_1000.yaml" "$fixtures/openapi.yaml" "$fixtures/malformed.yaml" "$fixtures/duplicate_step.yaml" > "$evidence/inputs.bytes"
printf '%s\n' 'bench profile; default features; host target from rustc -Vv; Rust default system allocator; locked Criterion 0.5.1 with workspace html_reports' > "$evidence/build.settings"
printf '%s\n' "RUSTFLAGS=${RUSTFLAGS-<unset>}" "CARGO_BUILD_TARGET=${CARGO_BUILD_TARGET-<unset>}" "CARGO_ENCODED_RUSTFLAGS=${CARGO_ENCODED_RUSTFLAGS-<unset>}" > "$evidence/build.overrides"
printf '%s\n' 'Criterion defaults: sample-size=100; warm-up-time=3s; measurement-time=5s; nresamples=100000; confidence-level=0.95; significance-level=0.05; noise-threshold=0.01; sampling-mode=Auto; no CLI overrides' > "$evidence/criterion.settings"
```

Record any relevant Cargo configuration/profile override alongside these files;
do not describe an overridden build as the defaults above. Record any manual
power or concurrent-load observations. Host provenance commands must succeed;
if the sandbox blocks a read, collect that read with the required permission
before interpreting the measurements. Never substitute guessed hardware or
power values. Keep local machine identifiers private when sharing artifacts.
Prefer a committed reviewed candidate. If unrelated work makes the checkout
dirty, identify it and verify that benchmark inputs and executable match the
reviewed candidate; `source.diff` does not capture untracked file contents.

## Smoke, then separate genuine runs

Smoke is a correctness/target check and is never performance evidence. These
direct executable invocations need `--bench` to select genuine measurement;
`--test` selects Criterion's smoke mode. Each `CRITERION_HOME` must be absent
before creation so prior samples cannot be reused as this invocation's output.

```sh
mkdir "$evidence/smoke"
printf '%s\n' "CRITERION_HOME=$evidence/smoke/native $binary --test" > "$evidence/smoke/command"
if CRITERION_HOME="$evidence/smoke/native" "$binary" --test > "$evidence/smoke/stdout" 2> "$evidence/smoke/stderr"; then
  smoke_status=0
else
  smoke_status=$?
fi
printf '%s\n' "$smoke_status" > "$evidence/smoke/exit"
test "$smoke_status" -eq 0
```

Require all three smoke series and both controls to pass, then run the following
block twice, first with `run=run_01`, then with `run=run_02`. The fixed order is
the same-host repeat control; keep the two invocations separate.

```sh
run=run_01
mkdir "$evidence/$run"
shasum -a 256 -c "$evidence/inputs.sha256" > "$evidence/$run/hash.check"
date -u '+%Y-%m-%dT%H:%M:%SZ' > "$evidence/$run/start.utc"
pmset -g batt > "$evidence/$run/power.before"
uptime > "$evidence/$run/load.before"
printf '%s\n' "CRITERION_HOME=$evidence/$run/native $binary --bench" > "$evidence/$run/command"
if CRITERION_HOME="$evidence/$run/native" "$binary" --bench > "$evidence/$run/stdout" 2> "$evidence/$run/stderr"; then
  run_status=0
else
  run_status=$?
fi
printf '%s\n' "$run_status" > "$evidence/$run/exit"
date -u '+%Y-%m-%dT%H:%M:%SZ' > "$evidence/$run/end.utc"
pmset -g batt > "$evidence/$run/power.after"
uptime > "$evidence/$run/load.after"
shasum -a 256 -c "$evidence/inputs.sha256" > "$evidence/$run/hash.after"
test "$run_status" -eq 0
```

Stop immediately on hash/provenance/process failure; retain the directory marked
invalid. Keep `set -e` enabled for the entire protocol. The `if` blocks preserve process
exit codes even under `set -e`. Never reuse a failed or partial directory, even
if it contains estimates. Require the final pass marker, five successful
preflight/control outcomes, all three series' accounting with failed=0, and
complete native outputs for all three series before reporting success.

Default runs take longer than the nominal three warmup plus five measurement
seconds per case because checking, scheduling, analysis and reporting also take
time. If a bounded local experiment overrides native CLI settings such as
`--sample-size 10 --warm-up-time 0.1 --measurement-time 0.2 --nresamples 1000`,
record the exact settings in `criterion.settings` and both command records,
apply identical overrides to both genuine runs, and label the experiment.
Requested durations are scheduling inputs, not hard wall-time budgets.

Retain complete native directories: `new/sample.json` contains sampled counts
and summed durations, `new/estimates.json` retains native estimates and confidence
intervals, and `new/tukey.json` retains outlier fences; stdout and HTML reports
retain Criterion's outlier labels. Keep all samples and outliers. A native
sample may aggregate many invocations; dividing sample duration by its count is
not an independent invocation distribution or a per-invocation p95. These are
not the workload assessment's separate twenty workflow invocations.

Compare each series' native estimates, intervals and noise across the original
and repeat, without inventing a threshold or pooling runs. For later paired
revision comparisons, alternate recorded invocation order (A, B, B, A), build
and identify each revision's exact executable first, preserve every invocation
separately and keep the host/configuration fixed. Never pool machines,
architectures or operating systems. Source indexing, HTTP/workflow execution,
CLI startup, CPU/RSS and runtime-worker settings have separate metric owners.
Regenerable evidence and reports stay under ignored `target/`; only the six
directly consumed YAML snapshots are committed inputs.
