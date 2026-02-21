#![forbid(unsafe_code)]

//! Stdio adapter loop for the internal debugger protocol.

mod session;
mod stdio;

pub use session::Session;
pub use stdio::run_stdio;

/// Stable internal debug adapter marker.
pub const INTERNAL_DEBUG_ADAPTER_VERSION: &str = "v1";
