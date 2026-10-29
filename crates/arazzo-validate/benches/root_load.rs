#[path = "support/root_load.rs"]
mod support;

use std::hint::black_box;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use arazzo_validate::parse_bytes_with_diagnostics;
use criterion::{BenchmarkId, Criterion};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("root_load INVALID invocation: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut criterion = Criterion::default().configure_from_args();
    let fixtures = support::Fixtures::load()
        .map_err(|error| format!("preparation requested=0 successful=0 failed=1: {error}"))?;
    fixtures.preflight()?;
    let mut group = criterion.benchmark_group("root_load");
    for case in &fixtures.roots {
        let mut requested = 0_u128;
        let mut successful = 0_u128;
        let label = format!("steps_{}_bytes_{}", case.steps, case.bytes.len());
        group.bench_function(BenchmarkId::from_parameter(&label), |bencher| {
            bencher.iter_custom(|iterations| {
                requested += u128::from(iterations);
                let mut elapsed = Duration::ZERO;
                for iteration in 0..iterations {
                    let bytes = black_box(case.bytes.as_slice());
                    let start = Instant::now();
                    let result = parse_bytes_with_diagnostics(bytes);
                    let interval = start.elapsed();
                    if let Err(error) = case.verify(&result) {
                        eprintln!(
                            "root_load INVALID invocation case={} callback_iteration={} requested={requested} successful={successful} failed=1 unattempted={}: {error}",
                            case.name, iteration + 1, requested - successful - 1
                        );
                        std::process::exit(1);
                    }
                    drop(result);
                    successful += 1;
                    elapsed += interval;
                }
                elapsed
            });
        });
        eprintln!(
            "root_load accounting case={} requested={requested} successful={successful} failed=0 unattempted=0",
            case.name
        );
    }
    group.finish();
    criterion.final_summary();
    eprintln!("root_load PASS invocation: complete preflight and all requested iterations checked");
    Ok(())
}
