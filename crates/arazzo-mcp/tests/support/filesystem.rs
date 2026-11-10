#![allow(clippy::unwrap_used, reason = "Fixture setup and assertions")]

use super::{extract_tool_text, is_tool_error};
use arazzo_mcp::state::ServerState;
use serde_json::Value;
use std::path::Path;

pub(super) fn rooted_file_state(path: &str) -> ServerState {
    ServerState::load(
        &[],
        Some(vec![Path::new(path)
            .parent()
            .unwrap()
            .to_string_lossy()
            .into_owned()]),
    )
    .unwrap()
}

pub(super) fn generation_tool_result(response: &Value) -> Value {
    assert!(!is_tool_error(response), "{response}");
    extract_tool_text(response).unwrap()
}

pub(super) fn source_file_path(url: &str) -> &str {
    let path = url.strip_prefix("file://").unwrap_or(url);
    if cfg!(windows) {
        path.trim_start_matches('/')
    } else {
        path
    }
}

pub(super) fn rooted_state_from_spec(path: &str, spec: arazzo_spec::ArazzoSpec) -> ServerState {
    let roots = spec
        .source_descriptions
        .iter()
        .map(|source| {
            Path::new(source_file_path(&source.url))
                .parent()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let mut state = ServerState::load(&[], Some(roots)).unwrap();
    state.specs.push(arazzo_mcp::state::LoadedSpec {
        file_path: path.into(),
        spec,
    });
    state
}

pub(super) fn file_policy_notice() -> String {
    "arazzo-mcp: file policy mode=denied, roots=0\n".to_string()
}
