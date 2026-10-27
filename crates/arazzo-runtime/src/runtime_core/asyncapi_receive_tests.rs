#![allow(clippy::expect_used, clippy::unwrap_used)] // Fixture construction and expected outcomes use explicit failure labels.
#![allow(clippy::field_reassign_with_default)] // Each rule mutates one fragment of the default document.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use arazzo_spec::{parse_unvalidated_bytes, ArazzoSpec, Step};
use serde_json::{json, Value};

use super::asyncapi_receive::{project_receive_target, ReceiveTarget};
use super::kafka::SaslMechanism;
use super::RuntimeErrorKind;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "arazzo-asyncapi-receive-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create fixture directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, data: &str) {
        fs::write(self.0.join(name), data).expect("write fixture source");
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove fixture directory");
    }
}

const CHANNEL_PATH: &str = "{$sourceDescriptions.orders.url}#/channels/orderPlaced";
const OPERATION_ID: &str = "$sourceDescriptions.orders.receiveOrderPlaced";

/// An AsyncAPI document assembled from fragments so each rule varies one
/// thing. `servers`, `channel_extra`, `operations`, `components`, and
/// `preamble` are YAML snippets at their natural indentation.
struct Doc {
    preamble: String,
    servers: String,
    address: String,
    messages: String,
    channel_extra: String,
    operations: String,
    components: String,
}

impl Default for Doc {
    fn default() -> Self {
        Self {
            preamble: String::new(),
            servers: "  local:\n    host: 127.0.0.1:19092\n    protocol: kafka\n".into(),
            address: "orders.placed".into(),
            messages: "      orderPlaced:\n        contentType: application/json\n".into(),
            channel_extra: String::new(),
            operations: "  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n".into(),
            components: String::new(),
        }
    }
}

impl Doc {
    fn render(&self) -> String {
        format!(
            "asyncapi: 3.0.0\n{}info:\n  title: Orders\n  version: 1.0.0\nservers:\n{}channels:\n  orderPlaced:\n    address: {}\n    messages:\n{}{}operations:\n{}{}",
            self.preamble,
            self.servers,
            self.address,
            self.messages,
            self.channel_extra,
            self.operations,
            if self.components.is_empty() {
                String::new()
            } else {
                format!("components:\n{}", self.components)
            },
        )
    }
}

fn arazzo(step: Value, source_type: &str) -> ArazzoSpec {
    let root = json!({
        "arazzo": "1.1.0",
        "info": {"title": "receive fixture", "version": "1"},
        "sourceDescriptions": [{"name": "orders", "type": source_type, "url": "./orders.asyncapi.yaml"}],
        "workflows": [{"workflowId": "flow", "steps": [step]}]
    });
    parse_unvalidated_bytes(&serde_json::to_vec(&root).expect("serialize")).expect("parse Arazzo")
}

fn channel_step() -> Value {
    json!({"stepId": "await", "channelPath": CHANNEL_PATH, "action": "receive"})
}

fn operation_step() -> Value {
    json!({"stepId": "await", "operationId": OPERATION_ID, "action": "receive"})
}

fn project_with(
    doc: &Doc,
    step: Value,
    source_type: &str,
    selection: Option<&str>,
) -> Result<ReceiveTarget, super::RuntimeError> {
    let dir = FixtureDir::new();
    dir.write("orders.asyncapi.yaml", &doc.render());
    let spec = arazzo(step, source_type);
    let step: Step = spec.workflows[0].steps[0].clone();
    project_receive_target(&spec, &step, Some(dir.path()), selection)
}

fn project(doc: &Doc, step: Value) -> Result<ReceiveTarget, super::RuntimeError> {
    project_with(doc, step, "asyncapi", None)
}

fn expect_unsupported(result: Result<ReceiveTarget, super::RuntimeError>, needle: &str) {
    let err = result.expect_err("expected an unsupported refusal");
    assert_eq!(
        err.kind,
        RuntimeErrorKind::UnsupportedAsyncApiTransport,
        "{}",
        err.message
    );
    assert!(err.message.contains(needle), "{}", err.message);
}

fn expect_config(result: Result<ReceiveTarget, super::RuntimeError>, needle: &str) {
    let err = result.expect_err("expected a configuration refusal");
    assert_eq!(
        err.kind,
        RuntimeErrorKind::MessageTransportConfig,
        "{}",
        err.message
    );
    assert!(err.message.contains(needle), "{}", err.message);
}

// ---- rule 1: step form -------------------------------------------------

#[test]
fn both_target_forms_project_the_same_target() {
    let doc = Doc::default();
    let by_channel = project(&doc, channel_step()).expect("channelPath form");
    let by_operation = project(&doc, operation_step()).expect("operationId form");
    assert_eq!(by_channel.topic, "orders.placed");
    assert_eq!(by_channel.channel_key, "orderPlaced");
    assert_eq!(by_channel.server_key, "local");
    assert_eq!(by_channel.endpoint.identity.host, "127.0.0.1:19092");
    assert_eq!(by_channel.endpoint.identity.source_name, "orders");
    assert!(!by_channel.endpoint.tls_required);
    assert_eq!(by_channel.endpoint.sasl, None);
    let mut by_operation = by_operation;
    by_operation.document.clone_from(&by_channel.document);
    assert_eq!(by_channel, by_operation);
}

#[test]
fn send_absent_action_correlation_and_request_body_are_refused() {
    let doc = Doc::default();
    expect_unsupported(
        project(
            &doc,
            json!({"stepId": "s", "channelPath": CHANNEL_PATH, "action": "send"}),
        ),
        "617",
    );
    expect_unsupported(
        project(&doc, json!({"stepId": "s", "channelPath": CHANNEL_PATH})),
        "must declare action: receive",
    );
    expect_unsupported(
        project(
            &doc,
            json!({"stepId": "s", "channelPath": CHANNEL_PATH, "action": "receive", "correlationId": "$inputs.id"}),
        ),
        "correlationId",
    );
    expect_unsupported(
        project(
            &doc,
            json!({"stepId": "s", "channelPath": CHANNEL_PATH, "action": "receive", "requestBody": {"payload": {}}}),
        ),
        "requestBody",
    );
}

// ---- rule 2: target form -----------------------------------------------

#[test]
fn operation_form_refuses_mismatched_action_ref_operations_and_executable_fields() {
    let mut doc = Doc::default();
    doc.operations = "  receiveOrderPlaced:\n    action: send\n    channel:\n      $ref: '#/channels/orderPlaced'\n".into();
    expect_unsupported(project(&doc, operation_step()), "action \"send\"");

    doc.operations = "  receiveOrderPlaced:\n    $ref: '#/components/operations/x'\n".into();
    expect_unsupported(project(&doc, operation_step()), "Reference Object");

    for field in ["security: []", "traits: []", "reply: {}"] {
        doc.operations = format!(
            "  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n    {field}\n"
        );
        expect_unsupported(project(&doc, operation_step()), "declares `");
    }

    doc.operations =
        "  other:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n"
            .into();
    expect_config(project(&doc, operation_step()), "is absent");
}

#[test]
fn operation_messages_must_list_every_channel_message() {
    let mut doc = Doc::default();
    doc.messages = "      orderPlaced:\n        contentType: application/json\n      orderCancelled:\n        contentType: application/json\n".into();
    doc.operations = "  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n    messages:\n      - $ref: '#/channels/orderPlaced/messages/orderPlaced'\n      - $ref: '#/channels/orderPlaced/messages/orderCancelled'\n".into();
    project(&doc, operation_step()).expect("complete list is admitted");

    doc.operations = "  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n    messages:\n      - $ref: '#/channels/orderPlaced/messages/orderPlaced'\n".into();
    expect_unsupported(project(&doc, operation_step()), "omits channel message(s)");
}

#[test]
fn operation_form_needs_a_source_qualified_id_naming_an_asyncapi_source() {
    let doc = Doc::default();
    expect_config(
        project(
            &doc,
            json!({"stepId": "s", "operationId": "receiveOrderPlaced", "action": "receive"}),
        ),
        "source-qualified",
    );
    expect_unsupported(
        project_with(&doc, operation_step(), "openapi", None),
        "is of type \"openapi\"",
    );
    expect_config(
        project(
            &doc,
            json!({"stepId": "s", "operationId": "$sourceDescriptions.shop.receiveOrderPlaced", "action": "receive"}),
        ),
        "not declared",
    );
}

// ---- rule 3: address ---------------------------------------------------

#[test]
fn address_must_be_a_literal_topic() {
    let mut doc = Doc::default();
    doc.address = "null".into();
    expect_unsupported(
        project(&doc, channel_step()),
        "MUST be interpreted as unknown",
    );
    doc.address = "'orders.{region}'".into();
    expect_unsupported(project(&doc, channel_step()), "Channel Address Expressions");
    doc.address = "orders.placed".into();
    doc.channel_extra = "    parameters:\n      region:\n        enum: [eu]\n".into();
    expect_unsupported(project(&doc, channel_step()), "parameters");
}

// ---- rules 4–6: servers ------------------------------------------------

#[test]
fn spec_kafka_server_example_is_accepted_and_kafka_secure_requires_tls() {
    let mut doc = Doc::default();
    // AsyncAPI 3.0.0 Server Object example, verbatim.
    doc.servers = "  production:\n    host: kafka.in.mycompany.com:9092\n    description: Production Kafka broker.\n    protocol: kafka\n    protocolVersion: '3.2'\n".into();
    let target = project(&doc, channel_step()).expect("spec example");
    assert_eq!(target.endpoint.identity.host, "kafka.in.mycompany.com:9092");
    assert_eq!(target.server_key, "production");

    doc.servers = "  secure:\n    host: broker.example:9093\n    protocol: kafka-secure\n".into();
    let target = project(&doc, channel_step()).expect("kafka-secure");
    assert!(target.endpoint.tls_required);
}

#[test]
fn channel_servers_subset_root_fallback_and_selection() {
    let mut doc = Doc::default();
    doc.servers = "  dev:\n    host: dev:9092\n    protocol: kafka\n  prod:\n    host: prod:9092\n    protocol: kafka\n  mq:\n    host: mq:5672\n    protocol: amqp\n".into();
    expect_config(project(&doc, channel_step()), "several Kafka servers");

    let picked = project_with(&doc, channel_step(), "asyncapi", Some("prod")).expect("selection");
    assert_eq!(picked.endpoint.identity.host, "prod:9092");
    assert_eq!(picked.server_key, "prod");

    expect_config(
        project_with(&doc, channel_step(), "asyncapi", Some("mq")),
        "eligible Kafka servers",
    );

    doc.channel_extra = "    servers:\n      - $ref: '#/servers/dev'\n".into();
    let subset = project(&doc, channel_step()).expect("channel subset");
    assert_eq!(subset.server_key, "dev");

    doc.channel_extra = "    servers:\n      - $ref: '#/servers/mq'\n".into();
    expect_config(project(&doc, channel_step()), "no Kafka server");

    doc.channel_extra = String::new();
    doc.servers = "  mq:\n    host: mq:5672\n    protocol: amqp\n".into();
    expect_config(project(&doc, channel_step()), "mq (amqp)");
}

#[test]
fn host_variables_substitute_defaults_only_and_pathname_is_refused() {
    let mut doc = Doc::default();
    doc.servers = "  local:\n    host: '{host}:{port}'\n    protocol: kafka\n    variables:\n      host:\n        default: broker.example\n      port:\n        default: '9094'\n        enum: ['9092', '9094']\n".into();
    let target = project(&doc, channel_step()).expect("variables");
    assert_eq!(target.endpoint.identity.host, "broker.example:9094");

    doc.servers = "  local:\n    host: '{host}:9092'\n    protocol: kafka\n".into();
    expect_config(
        project(&doc, channel_step()),
        "not declared under variables",
    );

    doc.servers = "  local:\n    host: '{host}:9092'\n    protocol: kafka\n    variables:\n      host:\n        enum: [a]\n".into();
    expect_config(project(&doc, channel_step()), "has no default");

    doc.servers = "  local:\n    host: '{port}'\n    protocol: kafka\n    variables:\n      port:\n        default: '1'\n        enum: ['2']\n".into();
    expect_config(project(&doc, channel_step()), "not in");

    doc.servers =
        "  local:\n    host: broker:9092\n    pathname: /vhost\n    protocol: kafka\n".into();
    expect_unsupported(project(&doc, channel_step()), "pathname");
}

// ---- rule 7: security --------------------------------------------------

#[test]
fn security_schemes_select_the_first_supported_mechanism() {
    let mut doc = Doc::default();
    for (type_, expected) in [
        ("plain", SaslMechanism::Plain),
        ("scramSha256", SaslMechanism::ScramSha256),
        ("scramSha512", SaslMechanism::ScramSha512),
    ] {
        doc.servers = format!(
            "  local:\n    host: broker:9092\n    protocol: kafka\n    security:\n      - type: {type_}\n"
        );
        let target = project(&doc, channel_step()).expect(type_);
        assert_eq!(target.endpoint.sasl, Some(expected), "{type_}");
    }

    doc.servers = "  local:\n    host: broker:9092\n    protocol: kafka\n    security:\n      - $ref: '#/components/securitySchemes/sasl'\n".into();
    doc.components = "  securitySchemes:\n    sasl:\n      type: scramSha256\n".into();
    let target = project(&doc, channel_step()).expect("component ref");
    assert_eq!(target.endpoint.sasl, Some(SaslMechanism::ScramSha256));

    doc.servers = "  local:\n    host: broker:9092\n    protocol: kafka\n    security:\n      - type: X509\n      - type: scramSha512\n".into();
    let target = project(&doc, channel_step()).expect("first supported wins");
    assert_eq!(target.endpoint.sasl, Some(SaslMechanism::ScramSha512));

    doc.servers = "  local:\n    host: broker:9092\n    protocol: kafka\n    security:\n      - type: oauth2\n        flows: {}\n".into();
    expect_unsupported(project(&doc, channel_step()), "all unsupported");

    doc.servers = "  local:\n    host: broker:9092\n    protocol: kafka\n    security: []\n".into();
    let target = project(&doc, channel_step()).expect("empty list");
    assert_eq!(target.endpoint.sasl, None);
}

// ---- rule 8: content types ---------------------------------------------

#[test]
fn effective_content_type_follows_message_then_traits_then_default() {
    let mut doc = Doc::default();
    doc.messages = "      orderPlaced:\n        contentType: text/plain\n".into();
    expect_unsupported(project(&doc, channel_step()), "text/plain");

    doc.messages =
        "      orderPlaced:\n        contentType: application/vnd.acme+json; charset=utf-8\n"
            .into();
    project(&doc, channel_step()).expect("+json suffix");

    doc.preamble = "defaultContentType: text/plain\n".into();
    doc.messages = "      orderPlaced:\n        contentType: application/json\n".into();
    project(&doc, channel_step()).expect("explicit json beats a text default");

    doc.messages = "      orderPlaced:\n        name: OrderPlaced\n".into();
    expect_unsupported(project(&doc, channel_step()), "text/plain");

    doc.preamble = String::new();
    doc.messages = "      orderPlaced:\n        traits:\n          - contentType: text/plain\n          - $ref: '#/components/messageTraits/json'\n".into();
    doc.components = "  messageTraits:\n    json:\n      contentType: application/json\n".into();
    project(&doc, channel_step()).expect("last trait wins");

    doc.messages = "      orderPlaced:\n        contentType: application/json\n        traits:\n          - contentType: text/plain\n".into();
    project(&doc, channel_step()).expect("message overrides its traits");

    doc.messages = "      orderPlaced:\n        $ref: '#/components/messages/placed'\n".into();
    doc.components = "  messages:\n    placed:\n      contentType: application/xml\n".into();
    expect_unsupported(project(&doc, channel_step()), "application/xml");

    doc.messages = "      orderPlaced:\n        $ref: '#/components/messages/missing'\n".into();
    expect_config(project(&doc, channel_step()), "missing components.messages");
}

// ---- rule 9: bindings --------------------------------------------------

#[test]
fn bindings_rule_refuses_wire_changing_fields_and_ignores_descriptive_ones() {
    let mut doc = Doc::default();
    doc.channel_extra = "    bindings:\n      kafka:\n        topic: orders.placed\n        partitions: 3\n        replicas: 1\n      amqp:\n        is: queue\n".into();
    doc.operations = "  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n    bindings:\n      kafka:\n        groupId: ignored\n        clientId: ignored\n".into();
    doc.servers = "  local:\n    host: 127.0.0.1:19092\n    protocol: kafka\n    bindings:\n      kafka:\n        schemaRegistryUrl: http://registry\n".into();
    project(&doc, operation_step()).expect("descriptive bindings are ignored");

    doc.channel_extra = "    bindings:\n      kafka:\n        topic: other-topic\n".into();
    expect_unsupported(project(&doc, channel_step()), "differs from the address");

    doc.channel_extra = String::new();
    doc.messages = "      orderPlaced:\n        contentType: application/json\n        bindings:\n          kafka:\n            schemaIdLocation: payload\n".into();
    expect_unsupported(project(&doc, channel_step()), "schemaIdLocation");
}

#[test]
fn document_and_channel_problems_keep_the_resolver_kinds() {
    let dir = FixtureDir::new();
    let spec = arazzo(channel_step(), "asyncapi");
    let step = spec.workflows[0].steps[0].clone();
    expect_config(
        project_receive_target(&spec, &step, Some(dir.path()), None),
        "local source file is absent",
    );

    let mut doc = Doc::default();
    doc.preamble = String::new();
    dir.write(
        "orders.asyncapi.yaml",
        &doc.render().replace("asyncapi: 3.0.0", "asyncapi: 2.6.0"),
    );
    expect_unsupported(
        project_receive_target(&spec, &step, Some(dir.path()), None),
        "outside supported 3.0.x",
    );
}

// ---- review round 1 counterexamples -------------------------------------

#[test]
fn trait_bindings_are_checked_even_when_the_message_sets_its_own_content_type() {
    let mut doc = Doc::default();
    doc.messages = "      orderPlaced:\n        contentType: application/json\n        traits:\n          - bindings:\n              kafka:\n                schemaIdLocation: payload\n".into();
    expect_unsupported(project(&doc, channel_step()), "schemaIdLocation");
}

#[test]
fn a_ref_with_sibling_properties_is_still_a_reference() {
    let mut doc = Doc::default();
    doc.operations = "  receiveOrderPlaced:\n    $ref: '#/components/operations/x'\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n".into();
    expect_unsupported(project(&doc, operation_step()), "Reference Object");

    doc.operations = Doc::default().operations;
    doc.servers = "  local:\n    $ref: '#/components/servers/x'\n    host: 127.0.0.1:19092\n    protocol: kafka\n".into();
    expect_unsupported(project(&doc, channel_step()), "Reference Object");

    doc.servers = Doc::default().servers;
    doc.messages = "      orderPlaced:\n        $ref: '#/components/messages/placed'\n        contentType: application/json\n".into();
    doc.components = "  messages:\n    placed:\n      contentType: text/plain\n".into();
    expect_unsupported(project(&doc, channel_step()), "text/plain");
}

#[test]
fn reference_fragments_are_percent_decoded_before_lookup() {
    let mut doc = Doc::default();
    doc.servers = "  foo:\n    host: foo:9092\n    protocol: kafka\n  '%66oo':\n    host: percent:9092\n    protocol: kafka\n".into();
    doc.channel_extra = "    servers:\n      - $ref: '#/servers/%66oo'\n".into();
    let target = project(&doc, channel_step()).expect("decoded reference");
    assert_eq!(target.server_key, "foo");
    assert_eq!(target.endpoint.identity.host, "foo:9092");

    doc.channel_extra = "    servers:\n      - $ref: '#/servers/%FF'\n".into();
    expect_config(project(&doc, channel_step()), "percent-encoded");
}

#[test]
fn referenced_bindings_are_refused_not_ignored() {
    let mut doc = Doc::default();
    doc.messages = "      orderPlaced:\n        contentType: application/json\n        bindings:\n          $ref: '#/components/messageBindings/framed'\n".into();
    expect_unsupported(
        project(&doc, channel_step()),
        "bindings is a Reference Object",
    );

    doc.messages = Doc::default().messages;
    doc.channel_extra = "    bindings:\n      $ref: '#/components/channelBindings/other'\n".into();
    expect_unsupported(
        project(&doc, channel_step()),
        "bindings is a Reference Object",
    );

    doc.channel_extra =
        "    bindings:\n      kafka:\n        $ref: '#/components/channelBindings/other'\n".into();
    expect_unsupported(
        project(&doc, channel_step()),
        "bindings.kafka is a Reference Object",
    );
}

#[test]
fn non_list_channel_servers_do_not_fall_back_to_all_servers() {
    let mut doc = Doc::default();
    doc.channel_extra = "    servers: {}\n".into();
    expect_config(
        project(&doc, channel_step()),
        "must be a list of Reference Objects",
    );
    doc.channel_extra = "    servers: dev\n".into();
    expect_config(
        project(&doc, channel_step()),
        "must be a list of Reference Objects",
    );
    doc.channel_extra = "    servers: []\n".into();
    project(&doc, channel_step()).expect("empty list means every server");
}

#[test]
fn operation_messages_referencing_undeclared_messages_are_refused() {
    let mut doc = Doc::default();
    doc.operations = "  receiveOrderPlaced:\n    action: receive\n    channel:\n      $ref: '#/channels/orderPlaced'\n    messages:\n      - $ref: '#/channels/orderPlaced/messages/orderPlaced'\n      - $ref: '#/channels/orderPlaced/messages/missing'\n".into();
    expect_config(project(&doc, operation_step()), "does not declare");
}

#[test]
fn repeated_server_references_count_once_and_selection_errors_list_only_eligible_keys() {
    let mut doc = Doc::default();
    doc.servers = "  dev:\n    host: dev:9092\n    protocol: kafka\n  mq:\n    host: mq:5672\n    protocol: amqp\n".into();
    doc.channel_extra =
        "    servers:\n      - $ref: '#/servers/dev'\n      - $ref: '#/servers/dev'\n".into();
    let target = project(&doc, channel_step()).expect("one server listed twice is one server");
    assert_eq!(target.server_key, "dev");

    doc.channel_extra = String::new();
    let err =
        project_with(&doc, channel_step(), "asyncapi", Some("mq")).expect_err("mq is not eligible");
    assert_eq!(err.kind, RuntimeErrorKind::MessageTransportConfig);
    assert!(err.message.contains("are dev"), "{}", err.message);
    assert!(!err.message.contains("mq (amqp)"), "{}", err.message);
}

#[test]
fn substituted_hosts_must_not_keep_any_brace() {
    let mut doc = Doc::default();
    doc.servers = "  local:\n    host: '{host}'\n    protocol: kafka\n    variables:\n      host:\n        default: '{missing}'\n".into();
    expect_config(project(&doc, channel_step()), "still contains a brace");
    doc.servers = "  local:\n    host: 'broker}:9092'\n    protocol: kafka\n".into();
    expect_config(project(&doc, channel_step()), "still contains a brace");
    doc.servers = "  local:\n    host: '{host:9092'\n    protocol: kafka\n".into();
    expect_config(project(&doc, channel_step()), "unterminated");
}

#[test]
fn step_form_and_component_refusals_carry_provenance() {
    let doc = Doc::default();
    let err = project(
        &doc,
        json!({"stepId": "s", "channelPath": CHANNEL_PATH, "action": "send"}),
    )
    .expect_err("send is refused");
    assert!(
        err.message.contains("sourceDescription \"orders\""),
        "{}",
        err.message
    );
    assert!(err.message.contains("step \"s\""), "{}", err.message);

    let mut doc = Doc::default();
    doc.messages = "      orderPlaced:\n        $ref: '#/components/messages/missing'\n".into();
    let err = project(&doc, channel_step()).expect_err("dangling message ref");
    assert!(
        err.message
            .contains("channel \"orderPlaced\" message \"orderPlaced\""),
        "{}",
        err.message
    );
}

// ---- review round 2 counterexamples -------------------------------------

#[test]
fn server_variable_references_are_followed_one_hop_and_their_siblings_ignored() {
    let mut doc = Doc::default();
    doc.servers = "  local:\n    host: '{host}:9092'\n    protocol: kafka\n    variables:\n      host:\n        $ref: '#/components/serverVariables/host'\n        default: wrong-broker\n".into();
    doc.components = "  serverVariables:\n    host:\n      default: right-broker\n".into();
    let target = project(&doc, channel_step()).expect("component variable");
    assert_eq!(target.endpoint.identity.host, "right-broker:9092");

    doc.components = String::new();
    expect_config(
        project(&doc, channel_step()),
        "missing components.serverVariables",
    );

    doc.servers = "  local:\n    host: '{host}:9092'\n    protocol: kafka\n    variables:\n      host:\n        $ref: '#/servers/local'\n        default: wrong-broker\n".into();
    expect_unsupported(
        project(&doc, channel_step()),
        "not a local #/components/serverVariables",
    );
}

#[test]
fn malformed_content_types_and_enums_are_document_errors_not_absences() {
    let mut doc = Doc::default();
    doc.messages = "      orderPlaced:\n        contentType: [application/xml]\n".into();
    expect_config(
        project(&doc, channel_step()),
        "contentType must be a string",
    );

    doc.messages = "      orderPlaced:\n        traits:\n          - contentType: {a: b}\n".into();
    expect_config(
        project(&doc, channel_step()),
        "contentType must be a string",
    );

    doc.messages = Doc::default().messages;
    doc.preamble = "defaultContentType: [text/plain]\n".into();
    expect_config(
        project(&doc, channel_step()),
        "defaultContentType must be a string",
    );

    doc.preamble = String::new();
    doc.servers = "  local:\n    host: '{env}:9092'\n    protocol: kafka\n    variables:\n      env:\n        default: production\n        enum: staging\n".into();
    expect_config(project(&doc, channel_step()), "enum must be a list");

    doc.servers = "  local:\n    host: '{env}:9092'\n    protocol: kafka\n    variables:\n      env:\n        default: 7\n".into();
    expect_config(project(&doc, channel_step()), "default must be a string");
}

// ---- review round 3 counterexamples -------------------------------------

#[test]
fn malformed_pathname_and_enum_elements_are_document_errors() {
    let mut doc = Doc::default();
    doc.servers =
        "  local:\n    host: broker:9092\n    protocol: kafka\n    pathname: ['/production']\n"
            .into();
    expect_config(project(&doc, channel_step()), "pathname must be a string");

    doc.servers = "  local:\n    host: broker:9092\n    protocol: kafka\n    pathname: ''\n".into();
    project(&doc, channel_step()).expect("an empty pathname is no pathname");

    doc.servers = "  local:\n    host: '{env}:9092'\n    protocol: kafka\n    variables:\n      env:\n        default: production\n        enum: [production, 7]\n".into();
    expect_config(
        project(&doc, channel_step()),
        "enum must be a list of strings",
    );
}
