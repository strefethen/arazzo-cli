#![allow(clippy::expect_used)] // Fixture construction and expected outcomes use explicit failure labels.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use arazzo_spec::{parse_unvalidated_bytes, ArazzoSpec, Step};
use serde_json::json;

use super::asyncapi_target::{resolve_local_channel, ResolveErrorKind};
use super::EngineBuilder;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "arazzo-asyncapi-target-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create fixture directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, data: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, data).expect("write fixture source");
        path
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove fixture directory");
    }
}

fn fixture(
    source: &str,
    source_type: &str,
    target: &str,
    action: Option<&str>,
    self_uri: Option<&str>,
) -> (ArazzoSpec, Step) {
    let mut root = json!({
        "arazzo": "1.1.0",
        "info": {"title": "channel fixture", "version": "1"},
        "sourceDescriptions": [{"name": "events", "type": source_type, "url": source}],
        "workflows": [{"workflowId": "flow", "steps": [{"stepId": "exchange", "channelPath": target}]}]
    });
    if let Some(action) = action {
        root["workflows"][0]["steps"][0]["action"] = json!(action);
    }
    if let Some(self_uri) = self_uri {
        root["$self"] = json!(self_uri);
    }
    let spec = parse_unvalidated_bytes(&serde_json::to_vec(&root).expect("serialize fixture"))
        .expect("parse Arazzo fixture");
    let step = spec.workflows[0].steps[0].clone();
    (spec, step)
}

fn target(pointer: &str) -> String {
    format!("{{$sourceDescriptions.events.url}}#{pointer}")
}

fn resolve(
    spec: &ArazzoSpec,
    step: &Step,
    dir: Option<&Path>,
) -> Result<super::asyncapi_target::ResolvedChannelTarget, super::asyncapi_target::ResolveError> {
    resolve_local_channel(spec, step, dir)
}

fn assert_kind(spec: &ArazzoSpec, step: &Step, dir: Option<&Path>, kind: ResolveErrorKind) {
    let err = resolve(spec, step, dir).expect_err("resolution must fail");
    assert_eq!(err.kind, kind, "{err:?}");
    assert_eq!(
        err.target,
        match &step.target {
            Some(arazzo_spec::StepTarget::ChannelPath(path)) => path,
            _ => "",
        }
    );
}

#[test]
fn resolves_json_send_and_yaml_receive_with_authored_channel_data() {
    let dir = FixtureDir::new();
    dir.write(
        "events.json",
        r##"{"asyncapi":"3.0.4","channels":{"orders":{"address":"orders/{region}","messages":{"placed":{"$ref":"#/components/messages/placed"}},"bindings":{"kafka":{"partitions":3}}}}}"##,
    );
    let (spec, step) = fixture(
        "events.json",
        "asyncapi",
        &target("/channels/orders"),
        Some("send"),
        None,
    );
    let selected = resolve(&spec, &step, Some(dir.path())).expect("resolve JSON channel");
    assert_eq!(selected.source_name, "events");
    assert!(selected.document.ends_with("events.json"));
    assert_eq!(selected.channel_key, "orders");
    assert_eq!(selected.channel_pointer, "/channels/orders");
    assert_eq!(selected.action.as_str(), "send");
    assert_eq!(selected.address.as_deref(), Some("orders/{region}"));
    assert!(selected.channel.get("bindings").is_some());
    assert!(selected.channel.get("messages").is_some());

    dir.write(
        "events.yaml",
        "asyncapi: 3.0.0\nchannels:\n  orders:\n    address: null\n    messages: {}\n",
    );
    let (spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("/channels/orders"),
        Some("receive"),
        None,
    );
    let selected = resolve(&spec, &step, Some(dir.path())).expect("resolve YAML channel");
    assert_eq!(selected.action.as_str(), "receive");
    assert_eq!(selected.address, None);
    assert!(selected.channel.get("messages").is_some());
}

#[test]
fn pointer_decoding_selects_key_not_address_and_unknown_address_is_allowed() {
    let dir = FixtureDir::new();
    dir.write("events.yaml", "asyncapi: 3.0.1-beta\nchannels:\n  'a/b~c':\n    title: Opaque\n  '%2F':\n    address: literal/percent\n  other:\n    address: literal/topic\n");
    let (spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("%2Fchannels%2Fa~1b~0c"),
        Some("send"),
        None,
    );
    let selected = resolve(&spec, &step, Some(dir.path())).expect("resolve escaped channel key");
    assert_eq!(selected.channel_key, "a/b~c");
    assert_eq!(selected.channel_pointer, "/channels/a~1b~0c");
    assert_eq!(selected.address, None);
    assert_eq!(
        selected
            .channel
            .get("title")
            .and_then(serde_yaml_ng::Value::as_str),
        Some("Opaque")
    );

    let (spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("/channels/%252F"),
        Some("send"),
        None,
    );
    let selected = resolve(&spec, &step, Some(dir.path())).expect("decode fragment once");
    assert_eq!(selected.channel_key, "%2F");
    assert_eq!(selected.address.as_deref(), Some("literal/percent"));

    let (spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("/channels/literal~1topic"),
        Some("send"),
        None,
    );
    assert_kind(
        &spec,
        &step,
        Some(dir.path()),
        ResolveErrorKind::MissingChannel,
    );
}

#[test]
fn absolute_file_self_overrides_retrieval_directory() {
    let actual = FixtureDir::new();
    let wrong = FixtureDir::new();
    actual.write(
        "events.json",
        r#"{"asyncapi":"3.0.0","channels":{"orders":{"address":"actual"}}}"#,
    );
    wrong.write(
        "events.json",
        r#"{"asyncapi":"3.0.0","channels":{"orders":{"address":"wrong"}}}"#,
    );
    let self_uri = url::Url::from_file_path(actual.path().join("workflow.arazzo.yaml"))
        .expect("fixture path is an absolute file URI");
    let (spec, step) = fixture(
        "events.json",
        "asyncapi",
        &target("/channels/orders"),
        Some("receive"),
        Some(self_uri.as_str()),
    );
    let selected = resolve(&spec, &step, Some(wrong.path())).expect("resolve from $self base");
    assert_eq!(selected.address.as_deref(), Some("actual"));
    assert!(selected
        .document
        .starts_with(actual.path().to_str().expect("UTF-8 fixture path")));
}

#[test]
fn missing_base_source_file_and_channel_are_distinct() {
    let dir = FixtureDir::new();
    let (spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("/channels/orders"),
        Some("send"),
        None,
    );
    assert_kind(&spec, &step, None, ResolveErrorKind::MissingBase);
    assert_kind(
        &spec,
        &step,
        Some(dir.path()),
        ResolveErrorKind::MissingFile,
    );
    dir.write("events.yaml", "asyncapi: 3.0.0\nchannels: {}\n");
    assert_kind(
        &spec,
        &step,
        Some(dir.path()),
        ResolveErrorKind::MissingChannel,
    );
    dir.write("events.yaml", "asyncapi: 3.0.0\n");
    assert_kind(
        &spec,
        &step,
        Some(dir.path()),
        ResolveErrorKind::MissingChannel,
    );

    let (mut spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("/channels/orders"),
        Some("send"),
        None,
    );
    spec.source_descriptions.clear();
    assert_kind(
        &spec,
        &step,
        Some(dir.path()),
        ResolveErrorKind::MissingSource,
    );
}

#[test]
fn wrong_source_type_action_and_pointer_are_rejected() {
    let dir = FixtureDir::new();
    dir.write("events.yaml", "asyncapi: 3.0.0\nchannels:\n  orders: {}\n");
    let (spec, step) = fixture(
        "events.yaml",
        "openapi",
        &target("/channels/orders"),
        Some("send"),
        None,
    );
    assert_kind(
        &spec,
        &step,
        Some(dir.path()),
        ResolveErrorKind::InvalidTarget,
    );
    let (spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("/channels/orders"),
        None,
        None,
    );
    assert_kind(
        &spec,
        &step,
        Some(dir.path()),
        ResolveErrorKind::InvalidTarget,
    );
    for pointer in ["/channels/~2", "/channels/%FF", "/channels/%GG"] {
        let (spec, step) = fixture(
            "events.yaml",
            "asyncapi",
            &target(pointer),
            Some("receive"),
            None,
        );
        assert_kind(
            &spec,
            &step,
            Some(dir.path()),
            ResolveErrorKind::InvalidTarget,
        );
    }
    let (mut spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("/channels/orders"),
        Some("send"),
        None,
    );
    spec.source_descriptions
        .push(spec.source_descriptions[0].clone());
    assert_kind(
        &spec,
        &step,
        Some(dir.path()),
        ResolveErrorKind::InvalidTarget,
    );
}

#[test]
fn malformed_documents_and_non_channel_targets_fail() {
    let dir = FixtureDir::new();
    let (spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("/channels/orders"),
        Some("send"),
        None,
    );
    for content in [
        "asyncapi: [",
        "{broken",
        "asyncapi: 3.0\nchannels: {}\n",
        "asyncapi: 3.0.0\nchannels: []\n",
        "asyncapi: '+3.0.0'\nchannels: {}\n",
        "asyncapi: '3.0.+1'\nchannels: {}\n",
    ] {
        dir.write("events.yaml", content);
        assert_kind(
            &spec,
            &step,
            Some(dir.path()),
            ResolveErrorKind::InvalidDocument,
        );
    }
    for content in [
        "asyncapi: 3.0.0\nchannels:\n  orders: 12\n",
        "asyncapi: 3.0.0\nchannels:\n  orders:\n    arbitrary: mapping\n",
        "asyncapi: 3.0.0\nchannels:\n  orders:\n    address: 12\n",
    ] {
        dir.write("events.yaml", content);
        assert_kind(
            &spec,
            &step,
            Some(dir.path()),
            ResolveErrorKind::InvalidDocument,
        );
    }
    dir.write(
        "events.yaml",
        "asyncapi: 3.0.0\ninfo:\n  title: not a channel\nchannels:\n  orders: {}\n",
    );
    let (spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("/info"),
        Some("send"),
        None,
    );
    assert_kind(
        &spec,
        &step,
        Some(dir.path()),
        ResolveErrorKind::UnsupportedCapability,
    );
}

#[test]
fn unsupported_versions_references_forms_and_remote_uri_fail_explicitly() {
    let dir = FixtureDir::new();
    let (spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("/channels/orders"),
        Some("receive"),
        None,
    );
    for version in ["2.6.0", "3.1.0", "4.0.0"] {
        dir.write(
            "events.yaml",
            &format!("asyncapi: {version}\nchannels:\n  orders: {{}}\n"),
        );
        assert_kind(
            &spec,
            &step,
            Some(dir.path()),
            ResolveErrorKind::UnsupportedCapability,
        );
    }
    dir.write(
        "events.yaml",
        "asyncapi: 3.0.0\nchannels:\n  orders:\n    $ref: '#/components/channels/orders'\n",
    );
    assert_kind(
        &spec,
        &step,
        Some(dir.path()),
        ResolveErrorKind::UnsupportedCapability,
    );
    let (spec, step) = fixture(
        "events.yaml",
        "asyncapi",
        &target("/components/channels/orders"),
        Some("receive"),
        None,
    );
    assert_kind(
        &spec,
        &step,
        Some(dir.path()),
        ResolveErrorKind::UnsupportedCapability,
    );

    // No listener is needed: non-file schemes are refused before acquisition.
    let (spec, step) = fixture(
        "http://127.0.0.1:9/events.yaml",
        "asyncapi",
        &target("/channels/orders"),
        Some("send"),
        None,
    );
    let err = resolve(&spec, &step, Some(dir.path())).expect_err("remote URI must fail");
    assert_eq!(err.kind, ResolveErrorKind::UnsupportedCapability);
    assert_eq!(err.source_name.as_deref(), Some("events"));
    assert_eq!(
        err.document.as_deref(),
        Some("http://127.0.0.1:9/events.yaml")
    );
}

#[test]
fn ordinary_engine_build_does_not_read_an_asyncapi_declaration() {
    let dir = FixtureDir::new();
    let (spec, _) = fixture(
        "absent.yaml",
        "asyncapi",
        &target("/channels/orders"),
        Some("send"),
        None,
    );
    // The private resolver reports the absent file; normal build still skips
    // asynchronous sources and succeeds without opening it.
    EngineBuilder::new(spec)
        .source_base_dir(dir.path())
        .build()
        .expect("engine skips absent AsyncAPI source");
}
