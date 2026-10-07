#![forbid(unsafe_code)]

//! Public-API integration harness for `arazzo-validate`.
//!
//! This is the crate's single integration-test binary for public-contract
//! tests; suites join it as submodules under `cases/` rather than as new
//! Cargo test binaries.

mod cases;
