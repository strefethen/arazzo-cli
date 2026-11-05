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
fn existing_readable_files_follow_the_native_platform_contract() {
    let fixture = Fixture::new();
    let path = fixture.write("report.json", OPENAPI);
    let state = fixture.state();
    assert_eq!(fs::read(&path).unwrap(), OPENAPI.as_bytes());
    if cfg!(target_os = "linux") {
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
    } else {
        call_all(&state, &path, "MCP_FILE_DENIED");
    }
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
    for path in [
        fixture.0.join("sub/../report.json"),
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
    assert_eq!(result["isError"], !cfg!(target_os = "linux"));
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
        if cfg!(target_os = "linux") && length <= 4 * 1024 * 1024 {
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
            call_all(
                &state,
                &path,
                if cfg!(target_os = "linux") {
                    "MCP_LIMIT_EXCEEDED"
                } else {
                    "MCP_FILE_DENIED"
                },
            );
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
    #[cfg(target_os = "linux")]
    {
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            fixture.0.join("fifo"),
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        call_all(&state, &fixture.0.join("fifo"), "MCP_FILE_DENIED");
        let device_state = ServerState::load(&[], Some(vec!["/dev".into()])).unwrap();
        call_all(&device_state, Path::new("/dev/null"), "MCP_FILE_DENIED");
    }
    fixture.write("single.yaml", OPENAPI);
    let moved = fixture.0.with_extension("moved");
    fs::rename(&fixture.0, &moved).unwrap();
    symlink(&outside, &fixture.0).unwrap();
    fs::write(outside.join("single.yaml"), CANARY).unwrap();
    let result =
        handlers::describe_openapi(&state, &json!({"file_path": fixture.0.join("single.yaml")}))
            .unwrap();
    assert_eq!(result["isError"], !cfg!(target_os = "linux"));
    assert!(!result.to_string().contains(CANARY));
}
