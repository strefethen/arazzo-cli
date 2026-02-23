#![forbid(unsafe_code)]

//! Debug Adapter Protocol (DAP) server for Arazzo workflow debugging.

mod dap;

pub use dap::run_dap_stdio;
