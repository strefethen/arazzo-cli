#![allow(clippy::unwrap_used, reason = "Fixture setup and assertions")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use arazzo_mcp::{handlers, state::ServerState};
use serde_json::{json, Value};

const CANARY: &str = "CANARY_private_filename_source_452ce";
const OPENAPI: &str = "openapi: 3.0.3\ninfo: {title: Test, version: '1'}\nservers: [{url: https://example.com}]\npaths:\n  /items:\n    get:\n      operationId: listItems\n      responses: {'200': {description: OK}}\n  /items/{itemId}:\n    get:\n      operationId: getItem\n      parameters: [{name: itemId, in: path, required: true, schema: {type: string}}]\n      responses: {'200': {description: OK}}\n";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "arazzo-files-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn state(&self) -> ServerState {
        ServerState::load(&[], Some(vec![self.0.to_str().unwrap().into()])).unwrap()
    }
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
}

fn call_all(state: &ServerState, path: &Path, code: &str) {
    for tool in [
        handlers::validate_spec,
        handlers::generate_workflow,
        handlers::describe_openapi,
    ] {
        let result = tool(state, &json!({"file_path": path.to_str().unwrap()})).unwrap();
        assert_eq!(result["isError"], true, "{result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        let error: Value = serde_json::from_str(text).unwrap();
        let message = if code == "MCP_FILE_DENIED" {
            "File access denied by server policy"
        } else {
            "Operation exceeded a server resource limit"
        };
        assert_eq!(error, json!({"code": code, "message": message}));
        assert!(!result.to_string().contains(CANARY));
        assert!(!text.contains(path.to_str().unwrap()));
    }
}

#[test]
fn all_direct_tools_default_to_denial_and_do_not_echo_the_target() {
    let fixture = Fixture::new();
    let path = fixture.write(CANARY, OPENAPI);
    call_all(&ServerState::empty(), &path, "MCP_FILE_DENIED");
    call_all(
        &ServerState::load(&[], None).unwrap(),
        &path,
        "MCP_FILE_DENIED",
    );
    call_all(
        &ServerState::load(&[], Some(vec![])).unwrap(),
        &path,
        "MCP_FILE_DENIED",
    );
}

#[test]
fn regular_files_inside_a_root_are_read_by_every_direct_tool() {
    let fixture = Fixture::new();
    let path = fixture.write("report.json", OPENAPI);
    let state = fixture.state();
    for tool in [handlers::generate_workflow, handlers::describe_openapi] {
        assert_eq!(
            tool(&state, &json!({"file_path": path})).unwrap()["isError"],
            false
        );
    }
    let result = handlers::validate_spec(&state, &json!({"file_path": path})).unwrap();
    assert_eq!(result["isError"], false);
    let parsed: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(parsed["valid"], false);
    assert_eq!(parsed["file"], path.to_str().unwrap());
    let dotted = fixture.0.join(".").join("report.json");
    assert_eq!(
        handlers::describe_openapi(&state, &json!({"file_path": dotted})).unwrap()["isError"],
        false
    );
}

#[test]
fn authored_parents_dot_descendants_and_sibling_prefixes_deny() {
    let fixture = Fixture::new();
    let state = fixture.state();
    fixture.write("report.json", OPENAPI);
    fixture.write(".hidden", CANARY);
    fixture.write(".hidden-dir/api.yaml", CANARY);
    fs::create_dir_all(fixture.0.join("sub")).unwrap();
    let sibling = fixture.0.with_extension("sibling");
    fs::create_dir_all(&sibling).unwrap();
    fs::write(sibling.join(CANARY), CANARY).unwrap();
    let root = fixture.0.to_str().unwrap();
    let separator = std::path::MAIN_SEPARATOR;
    for path in [
        PathBuf::from(format!(
            "{root}{separator}sub{separator}..{separator}report.json"
        )),
        fixture.0.join("../missing"),
        fixture.0.join(".hidden"),
        fixture.0.join(".hidden-dir/api.yaml"),
        fixture.0.join("missing/leaf"),
        fixture.0.join("absent"),
        sibling.join(CANARY),
        fixture.0.clone(),
    ] {
        call_all(&state, &path, "MCP_FILE_DENIED");
    }
    call_all(&state, Path::new("NUL\0target"), "MCP_FILE_DENIED");
}

#[test]
fn trusted_dot_root_and_literal_native_percent_text_are_not_descendants() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join(".trusted/root")).unwrap();
    let root = fixture.0.join(".trusted/root");
    let path = root.join("%2e%2e.json");
    fs::write(&path, OPENAPI).unwrap();
    let state = ServerState::load(&[], Some(vec![root.to_str().unwrap().into()])).unwrap();
    let result = handlers::describe_openapi(&state, &json!({"file_path": path})).unwrap();
    assert_eq!(result["isError"], false);
}

#[test]
fn document_limit_boundaries_are_enforced_by_every_direct_tool() {
    let fixture = Fixture::new();
    let state = fixture.state();
    for length in [4 * 1024 * 1024 - 1, 4 * 1024 * 1024, 4 * 1024 * 1024 + 1] {
        let mut content = OPENAPI.as_bytes().to_vec();
        content.extend_from_slice(b"#");
        content.resize(length, b' ');
        let path = fixture.write(&format!("boundary-{length}.yaml"), content);
        if length <= 4 * 1024 * 1024 {
            for tool in [
                handlers::validate_spec,
                handlers::generate_workflow,
                handlers::describe_openapi,
            ] {
                assert_eq!(
                    tool(&state, &json!({"file_path": path})).unwrap()["isError"],
                    false
                );
            }
        } else {
            call_all(&state, &path, "MCP_LIMIT_EXCEEDED");
        }
    }
}

#[cfg(unix)]
#[test]
fn links_special_files_and_pinned_root_replacement() {
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;
    let fixture = Fixture::new();
    let original = fixture.write("api.yaml", OPENAPI);
    fixture.write("sub/child", CANARY);
    let outside = fixture.0.with_extension("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("api"), CANARY).unwrap();
    symlink(&original, fixture.0.join("inside-link")).unwrap();
    symlink(&outside, fixture.0.join("outside-dir")).unwrap();
    symlink(fixture.0.join("sub"), fixture.0.join("inside-dir")).unwrap();
    symlink(outside.join("api"), fixture.0.join("outside-link")).unwrap();
    symlink(fixture.0.join("missing"), fixture.0.join("dangling")).unwrap();
    symlink("/proc/self/fd/0", fixture.0.join("magic")).unwrap();
    fs::hard_link(&original, fixture.0.join("hardlink")).unwrap();
    let other = fixture.write("external-hardlink", CANARY);
    fs::hard_link(other, outside.join("external-name")).unwrap();
    let _socket = UnixListener::bind(fixture.0.join("socket")).unwrap();
    let state = fixture.state();
    for name in [
        "api.yaml",
        "inside-link",
        "outside-dir/api",
        "inside-dir/child",
        "outside-link",
        "dangling",
        "magic",
        "hardlink",
        "external-hardlink",
        "socket",
    ] {
        call_all(&state, &fixture.0.join(name), "MCP_FILE_DENIED");
    }
    let fifo = fixture.0.join("fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    call_all(&state, &fifo, "MCP_FILE_DENIED");
    let device_state = ServerState::load(&[], Some(vec!["/dev".into()])).unwrap();
    call_all(&device_state, Path::new("/dev/null"), "MCP_FILE_DENIED");
    fixture.write("single.yaml", OPENAPI);
    let moved = fixture.0.with_extension("moved");
    fs::rename(&fixture.0, &moved).unwrap();
    symlink(&outside, &fixture.0).unwrap();
    fs::write(outside.join("single.yaml"), CANARY).unwrap();
    let result =
        handlers::describe_openapi(&state, &json!({"file_path": fixture.0.join("single.yaml")}))
            .unwrap();
    assert!(!result.to_string().contains(CANARY));
}

fn source_spec(url: &str) -> String {
    format!("arazzo: 1.1.0\ninfo: {{title: Sources, version: '1'}}\nsourceDescriptions: [{{name: api, url: '{url}', type: openapi}}]\nworkflows: [{{workflowId: list, steps: [{{stepId: list, operationId: listItems}}]}}]\n")
}

fn run_dry(fixture: &Fixture, spec: &str) -> (Value, String) {
    let spec_path = fixture.write("spec.arazzo.yaml", spec);
    let state = ServerState::load(
        &[spec_path.to_str().unwrap().into()],
        Some(vec![fixture.0.to_str().unwrap().into()]),
    )
    .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = handlers::run_workflow(
        &state,
        &json!({"workflow_id": "list", "dry_run": true}),
        &runtime,
    )
    .unwrap();
    let text = result["content"][0]["text"].as_str().unwrap().to_string();
    (result, text)
}

#[test]
fn referenced_openapi_source_outside_the_roots_is_denied_without_echo() {
    let fixture = Fixture::new();
    let outside = fixture.0.with_extension("outside-source");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join(CANARY), OPENAPI).unwrap();
    let outside_name = outside.file_name().unwrap().to_str().unwrap();
    let (result, text) = run_dry(
        &fixture,
        &source_spec(&format!("../{outside_name}/{CANARY}")),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        json!({"code": "MCP_FILE_DENIED", "message": "File access denied by server policy", "sourceDescription": "api"})
    );
    assert!(!text.contains(CANARY));
    assert!(!text.contains(outside.to_str().unwrap()));
}

#[test]
fn startup_default_roots_admit_a_source_beside_the_spec() {
    let fixture = Fixture::new();
    fixture.write("api.openapi.yaml", OPENAPI);
    let spec_path = fixture.write("spec.arazzo.yaml", source_spec("./api.openapi.yaml"));
    let state = ServerState::load(&[spec_path.to_str().unwrap().into()], None).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let result = handlers::run_workflow(
        &state,
        &json!({"workflow_id": "list", "dry_run": true}),
        &runtime,
    )
    .unwrap();
    assert_eq!(result["isError"], false, "{result}");
}

#[cfg(unix)]
#[test]
fn startup_default_roots_follow_the_typed_spec_spelling_only_for_its_real_directory() {
    use std::os::unix::fs::symlink;
    let run = |spec_path: &Path| {
        let state = ServerState::load(&[spec_path.to_str().unwrap().into()], None).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        handlers::run_workflow(
            &state,
            &json!({"workflow_id": "list", "dry_run": true}),
            &runtime,
        )
        .unwrap()
    };
    let fixture = Fixture::new();
    fixture.write("api.openapi.yaml", OPENAPI);
    fixture.write("spec.arazzo.yaml", source_spec("./api.openapi.yaml"));
    let alias = fixture.0.with_extension("spec-alias");
    symlink(&fixture.0, &alias).unwrap();
    let result = run(&alias.join("spec.arazzo.yaml"));
    assert_eq!(result["isError"], false, "{result}");

    let elsewhere = fixture.0.with_extension("spec-link-dir");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("api.openapi.yaml"), OPENAPI).unwrap();
    symlink(
        fixture.0.join("spec.arazzo.yaml"),
        elsewhere.join("spec.arazzo.yaml"),
    )
    .unwrap();
    let result = run(&elsewhere.join("spec.arazzo.yaml"));
    assert_eq!(result["isError"], true, "{result}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap(),
        json!({"code": "MCP_FILE_DENIED", "message": "File access denied by server policy", "sourceDescription": "api"})
    );
}

#[cfg(windows)]
#[test]
fn verbatim_canonical_roots_match_plain_disk_paths_and_still_contain() {
    fn plain(path: &Path) -> PathBuf {
        PathBuf::from(path.to_str().unwrap().strip_prefix(r"\\?\").unwrap())
    }
    let fixture = Fixture::new();
    fixture.write("report.json", OPENAPI);
    let outside = fixture.0.with_extension("outside-verbatim");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join(CANARY), CANARY).unwrap();
    let outside_name = outside.file_name().unwrap().to_str().unwrap();
    let root = fixture.0.to_str().unwrap();
    let state = fixture.state();
    for path in [
        plain(&fixture.0).join("report.json"),
        fixture.0.join("report.json"),
        PathBuf::from(format!(r"{root}\.\report.json")),
    ] {
        assert_eq!(
            handlers::describe_openapi(&state, &json!({"file_path": path})).unwrap()["isError"],
            false,
            "{}",
            path.display()
        );
    }
    for path in [
        plain(&outside).join(CANARY),
        outside.join(CANARY),
        PathBuf::from(format!(r"{root}\..\{outside_name}\{CANARY}")),
        PathBuf::from(format!(r"{root}\x/../../{outside_name}\{CANARY}")),
        PathBuf::from(format!(r"{root}\x/../D:/{CANARY}")),
    ] {
        call_all(&state, &path, "MCP_FILE_DENIED");
    }
}

#[test]
fn referenced_openapi_bytes_come_from_the_bounded_loader() {
    let fixture = Fixture::new();
    let mut content = OPENAPI.as_bytes().to_vec();
    content.extend_from_slice(b"#");
    content.resize(4 * 1024 * 1024 + 1, b' ');
    fixture.write("big.openapi.yaml", content);
    let spec = source_spec("./big.openapi.yaml");
    let native =
        arazzo_runtime::EngineBuilder::new(arazzo_validate::parse_bytes(spec.as_bytes()).unwrap())
            .source_base_dir(fixture.0.clone())
            .dry_run(true)
            .build();
    assert!(native.is_ok());
    let (result, text) = run_dry(&fixture, &spec);
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        text.contains("Operation exceeded a server resource limit"),
        "{text}"
    );
    assert!(!text.contains(fixture.0.to_str().unwrap()), "{text}");
}
