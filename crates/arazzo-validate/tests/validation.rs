#![forbid(unsafe_code)]

//! Public-API integration harness for `arazzo-validate`.
//!
//! This is the single integration-test binary the arazzo-validate
//! decomposition plan designates for public-contract tests; suites join it
//! as submodules under `cases/` rather than as new Cargo test binaries.

mod cases;
