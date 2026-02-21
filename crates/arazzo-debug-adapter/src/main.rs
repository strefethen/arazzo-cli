#![forbid(unsafe_code)]

use std::io::{self, BufReader, BufWriter};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());

    if let Err(err) = arazzo_debug_adapter::run_dap_stdio(&mut reader, &mut writer) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
