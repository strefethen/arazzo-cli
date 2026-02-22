#![forbid(unsafe_code)]

use std::io::{self, BufReader, BufWriter};

fn main() {
    let stdout = io::stdout();
    let reader = BufReader::new(io::stdin());
    let mut writer = BufWriter::new(stdout.lock());

    if let Err(err) = arazzo_debug_adapter::run_dap_stdio(reader, &mut writer) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
