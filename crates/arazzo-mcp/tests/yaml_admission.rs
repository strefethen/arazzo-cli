#![allow(clippy::unwrap_used, reason = "Fixture setup and assertions")]

use std::fs;
use std::path::PathBuf;

use arazzo_mcp::{handlers, state::ServerState};
use serde_json::{json, Value};

const ARAZZO: &str = "arazzo: 1.1.0\ninfo: {title: Bomb, version: '1'}\nsourceDescriptions:\n  - {name: api, url: ./api.yaml, type: openapi}\nworkflows:\n  - workflowId: wf\n    steps:\n      - {stepId: s, operationId: listItems}\n";
const OPENAPI: &str = "openapi: 3.0.3\ninfo: {title: Bomb, version: '1'}\nservers: [{url: https://example.com}]\npaths:\n  /items:\n    get:\n      operationId: listItems\n      responses: {'200': {description: OK}}\n";

fn alias_bomb(document: &str) -> String {
    let leaves = vec!["x"; 1500].join(", ");
    let level = |alias: &str| vec![alias; 40].join(", ");
    format!(
        "{document}x-bomb:\n  a: &a [{leaves}]\n  b: &b [{}]\n  c: &c [{}]\n  d: [{}]\n",
        level("*a"),
        level("*b"),
        level("*c"),
    )
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("arazzo-mcp-yaml-admission-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn write(&self, name: &str, contents: &str) -> String {
        assert!(
            contents.len() < 8 * 1024,
            "{name} is {} bytes",
            contents.len()
        );
        let path = self.0.join(name);
        fs::write(&path, contents).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn state(&self) -> ServerState {
        ServerState::load(&[], Some(vec![self.0.to_str().unwrap().into()])).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

#[test]
fn alias_bombs_are_ordinary_tool_results_naming_the_node_budget() {
    let fixture = Fixture::new();
    let state = fixture.state();
    let arazzo = fixture.write("bomb.arazzo.yaml", &alias_bomb(ARAZZO));
    let openapi = fixture.write("api.yaml", &alias_bomb(OPENAPI));

    let validated = handlers::validate_spec(&state, &json!({"file_path": arazzo})).unwrap();
    assert_eq!(validated["isError"], false, "{validated}");
    let report: Value = serde_json::from_str(text(&validated)).unwrap();
    assert_eq!(report["valid"], false, "{report}");
    assert_eq!(report["errors"][0]["source"], "parseYaml", "{report}");
    let message = report["errors"][0]["message"].as_str().unwrap();
    assert!(message.contains("YAML node budget exceeded"), "{message}");

    for tool in [handlers::describe_openapi, handlers::generate_workflow] {
        let result = tool(&state, &json!({"file_path": openapi})).unwrap();
        assert_eq!(result["isError"], true, "{result}");
        assert!(
            text(&result).contains("YAML node budget exceeded"),
            "{result}"
        );
    }
}
