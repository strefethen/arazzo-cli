//! Transport-trust flag plumbing shared by `run` and `test`: builds
//! `ClientConfig` transport fields and the structured warning entries,
//! and enforces the channel discipline (warnings on stderr only; the
//! squelch silences stderr text while structured entries persist).

use std::collections::BTreeSet;

use arazzo_runtime::{ClientConfig, TransportWarning, TransportWarningKind};

/// Transport flags accepted by `run` and `test`.
#[derive(Debug, Clone, Default)]
pub struct TransportFlags {
    pub insecure_hosts: Vec<String>,
    pub insecure_all: bool,
    pub allow_downgrade_redirects: bool,
    pub max_redirects: usize,
    /// `--no-transport-warnings` / `ARAZZO_NO_TRANSPORT_WARNINGS=1`:
    /// silences stderr warning text only.
    pub no_transport_warnings: bool,
}

impl TransportFlags {
    /// Applies the flags to a `ClientConfig`. `engine_stderr` controls
    /// whether the engine itself writes warning lines to stderr (the
    /// `test` command keeps the engine silent and prints deduplicated
    /// lines once, after all suites).
    pub fn apply(&self, cfg: &mut ClientConfig, engine_stderr: bool) {
        cfg.insecure_hosts = self.insecure_hosts.iter().cloned().collect::<BTreeSet<_>>();
        cfg.insecure_all = self.insecure_all;
        cfg.allow_downgrade_redirects = self.allow_downgrade_redirects;
        cfg.max_redirects = self.max_redirects;
        cfg.transport_warnings = engine_stderr && !self.no_transport_warnings;
    }

    /// Startup notice for live runs when verification exceptions are
    /// configured. Blanket `--insecure` gets the louder wording.
    pub fn startup_warning(&self) -> Option<TransportWarning> {
        if self.insecure_all {
            return Some(TransportWarning {
                kind: TransportWarningKind::InsecureAllHosts,
                hosts: Vec::new(),
                message: "TLS verification disabled for ALL hosts (--insecure)".to_string(),
            });
        }
        if self.insecure_hosts.is_empty() {
            return None;
        }
        let entries = self
            .insecure_hosts
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        Some(TransportWarning {
            kind: TransportWarningKind::InsecureHostsActive,
            message: format!(
                "TLS verification disabled for: {} (--insecure-host)",
                entries.join(", ")
            ),
            hosts: entries,
        })
    }
}

/// End-of-run note naming configured exceptions no request targeted.
pub fn unused_warning(unused: Vec<String>) -> Option<TransportWarning> {
    if unused.is_empty() {
        return None;
    }
    Some(TransportWarning {
        kind: TransportWarningKind::UnusedInsecureHosts,
        message: format!(
            "unused --insecure-host exception(s): {} (no request targeted them)",
            unused.join(", ")
        ),
        hosts: unused,
    })
}

/// Writes one warning line to stderr unless squelched. Never stdout.
pub fn eprint_warning(warning: &TransportWarning, squelched: bool) {
    if squelched {
        return;
    }
    match warning.kind {
        TransportWarningKind::InsecureAllHosts => eprintln!("WARNING: {}", warning.message),
        _ => eprintln!("warning: {}", warning.message),
    }
}
