use super::catalog::{HarnessError, Settings, CATALOG_VERSION};
use super::execution_oracle::OracleReport;
use super::protocol::Snapshot;
use serde::{Deserialize, Serialize};
use std::io::Write;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InvocationRole {
    Verification,
    Warmup,
    Measured,
    Empty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Actual {
    pub terminal_kind: String,
    pub runtime_code: Option<String>,
    pub output_file: Option<String>,
    pub event_file: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvidenceFile {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cleanup {
    pub release_ok: bool,
    pub active_connections: Option<u32>,
    pub runtime_shutdown_bounded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawRecord {
    pub format_version: u32,
    pub catalog_version: u32,
    pub case_id: Option<String>,
    pub manifest_id: Option<String>,
    pub invocation_id: String,
    pub role: InvocationRole,
    pub case_kind: String,
    pub settings: Option<Settings>,
    pub metric: String,
    pub elapsed_ns: Option<u64>,
    pub outcome: String,
    pub sample_eligible: bool,
    pub actual: Actual,
    pub evidence_files: Vec<EvidenceFile>,
    pub oracle: OracleReport,
    pub service: Option<Snapshot>,
    pub cleanup: Cleanup,
    pub failure: Option<HarnessError>,
}

impl RawRecord {
    pub fn invalid(invocation_id: String, role: InvocationRole) -> Self {
        Self {
            format_version: 1,
            catalog_version: CATALOG_VERSION,
            case_id: None,
            manifest_id: None,
            invocation_id,
            role,
            case_kind: "empty".into(),
            settings: None,
            metric: "empty_worker".into(),
            elapsed_ns: None,
            outcome: "invalid".into(),
            sample_eligible: false,
            actual: Actual {
                terminal_kind: "not_started".into(),
                runtime_code: None,
                output_file: None,
                event_file: None,
                error_message: None,
            },
            evidence_files: Vec::new(),
            oracle: OracleReport::default(),
            service: None,
            cleanup: Cleanup {
                release_ok: false,
                active_connections: None,
                runtime_shutdown_bounded: false,
            },
            failure: None,
        }
    }
}

pub(crate) fn classify_exit(record: &RawRecord) -> i32 {
    let sound = record.format_version == 1
        && record.catalog_version == CATALOG_VERSION
        && record.elapsed_ns.is_some()
        && record.failure.is_none()
        && record.oracle.passed
        && record.cleanup.release_ok
        && record.cleanup.runtime_shutdown_bounded;
    let catalog_identity = record
        .case_id
        .as_deref()
        .and_then(|id| super::catalog::case(id).ok())
        .is_some_and(|case| {
            record.manifest_id.as_deref() == Some(format!("catalog-1/{}", case.case_id).as_str())
                && record.settings.as_ref() == Some(&case.settings)
                && record.actual.terminal_kind == case.expected.terminal_kind
                && record.case_kind
                    == if case.control.is_some() {
                        "negative_control"
                    } else {
                        "positive"
                    }
                && record.metric
                    == if case.family == super::catalog::Family::Sources {
                        "source_build"
                    } else {
                        "workflow_completion"
                    }
                && record.cleanup.active_connections == Some(0)
                && if case.expected.terminal_kind == "success" {
                    record.actual.runtime_code.is_none() && record.actual.error_message.is_none()
                } else {
                    record.actual.runtime_code.is_some() && record.actual.error_message.is_some()
                }
        });
    let accepted = match record.outcome.as_str() {
        "valid" => {
            catalog_identity
                && record.case_kind == "positive"
                && record.case_id.is_some()
                && record.manifest_id.is_some()
                && record.settings.is_some()
                && record.role != InvocationRole::Empty
                && record.sample_eligible == (record.role == InvocationRole::Measured)
        }
        "verified_negative" => {
            catalog_identity
                && record.case_kind == "negative_control"
                && record.role == InvocationRole::Verification
                && !record.sample_eligible
        }
        "empty" => {
            record.case_kind == "empty"
                && record.role == InvocationRole::Empty
                && !record.sample_eligible
                && record.settings.is_none()
                && record.service.is_none()
        }
        _ => false,
    };
    if sound && accepted {
        0
    } else {
        1
    }
}

pub(crate) fn write_record(record: &RawRecord, mut out: impl Write) -> Result<(), HarnessError> {
    let mut bytes =
        serde_json::to_vec(record).map_err(|e| HarnessError::new("record", "encode", e))?;
    bytes.push(b'\n');
    out.write_all(&bytes)
        .and_then(|()| out.flush())
        .map_err(|e| HarnessError::new("record", "write", e))
}

#[allow(dead_code)]
pub(crate) fn read_record(bytes: &[u8]) -> Result<RawRecord, HarnessError> {
    super::protocol::decode_frame(bytes, 16 * 1024 * 1024)
}
