#![allow(clippy::unwrap_used, reason = "Fixture setup and assertions")]

use super::{extract_tool_text, is_tool_error};
use arazzo_mcp::state::ServerState;
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};

pub(super) fn native_dry_run_bytes(
    spec: &[u8],
    base: &Path,
    workflow: &str,
    inputs: BTreeMap<String, Value>,
) -> Value {
    let engine = arazzo_runtime::EngineBuilder::new(arazzo_validate::parse_bytes(spec).unwrap())
        .source_base_dir(base.to_path_buf())
        .dry_run(true)
        .build()
        .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = runtime.block_on(async { engine.execute(workflow, inputs).collect().await });
    assert!(result.outputs.is_ok(), "{:?}", result.outputs);
    json!({"kind": "dryRun", "requests": result.dry_run_requests().into_iter().cloned().map(arazzo_runtime::redacted_dry_run_request).collect::<Vec<_>>()})
}

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

pub(super) fn validation_tool_or_denial_and_byte_parser(result: Value, path: &str) -> Value {
    if cfg!(target_os = "linux") {
        return result;
    }
    assert_eq!(result["code"], "MCP_FILE_DENIED");
    let bytes = fs::read(path).unwrap();
    let diagnostics = |items: &[arazzo_validate::Diagnostic]| {
        items.iter().map(|item| json!({
        "source": "validation", "path": if item.path.is_empty() { Value::Null } else { json!(item.path) }, "message": item.message
    })).collect::<Vec<_>>()
    };
    match arazzo_validate::parse_bytes_with_diagnostics(&bytes) {
        Ok((spec, warnings)) => {
            json!({"valid": true, "workflows": spec.workflows.len(), "warnings": diagnostics(&warnings)})
        }
        Err(arazzo_validate::Error::Validation(report)) => {
            json!({"valid": false, "errors": diagnostics(&report.errors), "warnings": diagnostics(&report.warnings)})
        }
        Err(error) => panic!("unexpected byte parser failure: {error}"),
    }
}

pub(super) fn generation_tool_or_denial_and_byte_parser(response: &Value, path: &str) -> Value {
    let result = extract_tool_text(response).unwrap();
    if cfg!(target_os = "linux") {
        assert!(!is_tool_error(response), "{response}");
        return result;
    }
    assert!(is_tool_error(response));
    assert_eq!(result["code"], "MCP_FILE_DENIED");
    let openapi = arazzo_generate::parse_openapi_bytes(&fs::read(path).unwrap()).unwrap();
    let name = arazzo_generate::crud::guard_uri_scheme_ambiguity(
        Path::new(path).file_name().unwrap().to_str().unwrap(),
    );
    let generated = arazzo_generate::crud::generate_crud(&openapi, path, &name).unwrap();
    json!({"yaml": serde_yaml_ng::to_string(&generated.spec).unwrap()})
}

pub(super) fn rooted_state_from_spec(path: &str, spec: arazzo_spec::ArazzoSpec) -> ServerState {
    let roots = spec
        .source_descriptions
        .iter()
        .map(|source| {
            Path::new(&source.url)
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

pub(super) fn workflow_tool_or_denial_and_native_runtime(
    response: &Value,
    state: &ServerState,
    workflow: &str,
    dry_run: bool,
) -> Value {
    if cfg!(target_os = "linux") {
        return extract_tool_text(response).unwrap();
    }
    assert!(is_tool_error(response), "{response}");
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("File access denied by server policy"),
        "{response}"
    );
    let engine = arazzo_runtime::EngineBuilder::new(state.specs[0].spec.clone())
        .source_base_dir(std::env::current_dir().unwrap())
        .dry_run(dry_run)
        .build()
        .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result =
        runtime.block_on(async { engine.execute(workflow, BTreeMap::new()).collect().await });
    match &result.outputs {
        Ok(outputs) if !dry_run => json!({"kind":"success", "outputs":outputs}),
        Ok(_) => {
            json!({"kind":"dryRun", "requests":result.dry_run_requests().into_iter().cloned().map(arazzo_runtime::redacted_dry_run_request).collect::<Vec<_>>()})
        }
        Err(error) => json!({"code":error.code(), "error":error.message}),
    }
}

pub(super) fn file_policy_notice() -> String {
    format!("arazzo-mcp: file policy mode=denied, roots=0; dot-prefixed descendants denied; tool file reads {}\n", if cfg!(target_os = "linux") { "use secure Linux acquisition" } else { "denied on this platform" })
}
