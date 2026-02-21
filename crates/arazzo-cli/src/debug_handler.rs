use std::io::{self, BufReader, BufWriter};

pub fn run_debug_stdio() -> Result<(), String> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    arazzo_debug_adapter::run_stdio(&mut reader, &mut writer)
}
