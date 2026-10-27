//! Production admission of an AsyncAPI receive step onto a Kafka endpoint.
//!
//! Pure functions over the parsed document plus the existing local-file
//! binding in `asyncapi_target`; nothing here opens a socket. Every rule is
//! either a specification requirement (quoted at the site) or a documented
//! implementation choice from
//! `plans/assessments/arazzo-first-asyncapi-transport-decision.md` (D1–D4,
//! D11). Unsupported-but-valid document forms are refused with
//! [`RuntimeErrorKind::UnsupportedAsyncApiTransport`]; structural and
//! selection problems with [`RuntimeErrorKind::MessageTransportConfig`].
//! Every message names the source, the document, and the offending element.
//!
//! Reference Objects: AsyncAPI 3.0.0 says a Reference Object "is defined by
//! JSON Reference" and that other properties in the same object are ignored,
//! so any mapping carrying a `$ref` key is a reference here, whatever its
//! siblings. One local hop into `components` is followed for the objects
//! this module interprets; everything else that is reference-capable and
//! consequential (channels, operations, bindings) is refused rather than
//! guessed.
//!
//! Binding fields the D11 rule ignores (documented for the user guide):
//! every server binding, channel `kafka.partitions` / `kafka.replicas` /
//! `kafka.topicConfiguration`, operation `kafka.groupId` / `kafka.clientId`,
//! message `kafka.key`, and every non-`kafka` binding.

use std::collections::BTreeSet;
use std::path::Path;

use arazzo_spec::{
    classify_operation_id, classify_operation_path, ArazzoSpec, OperationIdTarget,
    OperationPathTarget, SourceType, Step, StepAction, StepTarget,
};
use serde_yaml_ng::Value as Yaml;

use super::asyncapi_target::{
    channel_from_root, load_local_asyncapi, resolve_local_channel, LoadedAsyncApi, ResolveError,
    ResolveErrorKind,
};
use super::kafka::{ConnectionIdentity, KafkaEndpoint, SaslMechanism};
use super::{RuntimeError, RuntimeErrorKind};

/// Everything the engine needs to arm a receive step on Kafka.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceiveTarget {
    pub(crate) source_name: String,
    pub(crate) document: String,
    pub(crate) channel_key: String,
    /// The channel `address`, verbatim: the Kafka topic (D1).
    pub(crate) topic: String,
    pub(crate) server_key: String,
    pub(crate) endpoint: KafkaEndpoint,
}

/// Message binding fields that change what is on the wire and are refused.
const REFUSED_MESSAGE_BINDING_FIELDS: &[&str] = &[
    "schemaIdLocation",
    "schemaIdPayloadEncoding",
    "schemaLookupStrategy",
];

/// Provenance carried into every diagnostic: the source description, and
/// the document once it is loaded (the target string before that).
struct Context {
    source_name: String,
    document: String,
}

impl Context {
    fn before_load(step: &Step) -> Self {
        let (source_name, document) = match &step.target {
            Some(StepTarget::ChannelPath(target)) => match classify_operation_path(target) {
                OperationPathTarget::SourcePointer { source_name, .. } => {
                    (source_name.to_string(), format!("target {target:?}"))
                }
                OperationPathTarget::Malformed(_) => {
                    ("?".to_string(), format!("target {target:?}"))
                }
            },
            Some(StepTarget::OperationId(target)) => match classify_operation_id(target) {
                OperationIdTarget::SourceQualified { source_name, .. } => {
                    (source_name.to_string(), format!("target {target:?}"))
                }
                _ => ("?".to_string(), format!("target {target:?}")),
            },
            Some(StepTarget::OperationPath(target)) => {
                ("?".to_string(), format!("target {target:?}"))
            }
            Some(StepTarget::WorkflowId(target)) => {
                ("?".to_string(), format!("workflowId {target:?}"))
            }
            None => ("?".to_string(), "no target".to_string()),
        };
        Self {
            source_name,
            document,
        }
    }

    fn unsupported(&self, detail: impl std::fmt::Display) -> RuntimeError {
        RuntimeError::new(
            RuntimeErrorKind::UnsupportedAsyncApiTransport,
            format!(
                "sourceDescription \"{}\" ({}): {detail}",
                self.source_name, self.document
            ),
        )
    }

    fn config(&self, detail: impl std::fmt::Display) -> RuntimeError {
        RuntimeError::new(
            RuntimeErrorKind::MessageTransportConfig,
            format!(
                "sourceDescription \"{}\" ({}): {detail}",
                self.source_name, self.document
            ),
        )
    }
}

fn resolve_error(err: ResolveError) -> RuntimeError {
    let kind = match err.kind {
        ResolveErrorKind::UnsupportedCapability => RuntimeErrorKind::UnsupportedAsyncApiTransport,
        _ => RuntimeErrorKind::MessageTransportConfig,
    };
    let mut message = String::new();
    if let Some(source) = &err.source_name {
        message.push_str(&format!("sourceDescription \"{source}\""));
        if let Some(document) = &err.document {
            message.push_str(&format!(" ({document})"));
        }
        message.push_str(": ");
    }
    message.push_str(&err.detail);
    if !err.target.is_empty() {
        message.push_str(&format!(" (target {:?})", err.target));
    }
    RuntimeError::new(kind, message)
}

/// Outcome of reading a local `#/<prefix><key>` reference.
enum LocalRef {
    /// The decoded, unescaped last token.
    Key(String),
    /// Not `#/<prefix>` followed by exactly one token.
    OtherShape,
    /// The fragment is not valid percent-encoded UTF-8.
    BadEncoding,
}

/// Reads a local JSON Reference whose fragment is `prefix` followed by one
/// more token. The fragment is percent-decoded first (RFC 3986), then the
/// RFC 6901 escapes are undone, so `#/channels/%66oo` names `foo`, never a
/// key literally spelled `%66oo`.
fn local_ref_key(reference: &str, prefix: &str) -> LocalRef {
    let Some(fragment) = reference.strip_prefix('#') else {
        return LocalRef::OtherShape;
    };
    let Ok(decoded) = percent_encoding::percent_decode_str(fragment).decode_utf8() else {
        return LocalRef::BadEncoding;
    };
    let Some(rest) = decoded.strip_prefix(prefix) else {
        return LocalRef::OtherShape;
    };
    if rest.is_empty() || rest.contains('/') {
        return LocalRef::OtherShape;
    }
    LocalRef::Key(rest.replace("~1", "/").replace("~0", "~"))
}

/// The `$ref` of a Reference Object. AsyncAPI 3.0.0 Reference Object: the
/// object "is defined by JSON Reference" — any mapping with a `$ref` key is a
/// reference, and its other properties are not interpreted.
fn ref_of(value: &Yaml) -> Option<&str> {
    value
        .as_mapping()?
        .get(Yaml::String("$ref".into()))?
        .as_str()
}

fn is_reference(value: &Yaml) -> bool {
    value
        .as_mapping()
        .is_some_and(|map| map.contains_key(Yaml::String("$ref".into())))
}

fn components<'a>(root: &'a Yaml, section: &str) -> Option<&'a Yaml> {
    root.get("components")?.get(section)
}

/// Follows one local `$ref` into `components.<section>`; nested references
/// are refused. `what` names the referring element for diagnostics.
fn deref_component<'a>(
    ctx: &Context,
    root: &'a Yaml,
    value: &'a Yaml,
    section: &str,
    what: &str,
) -> Result<&'a Yaml, RuntimeError> {
    if !is_reference(value) {
        return Ok(value);
    }
    let reference =
        ref_of(value).ok_or_else(|| ctx.config(format!("{what}: $ref must be a string")))?;
    let prefix = format!("/components/{section}/");
    let key = match local_ref_key(reference, &prefix) {
        LocalRef::Key(key) => key,
        LocalRef::BadEncoding => {
            return Err(ctx.config(format!(
                "{what}: $ref {reference:?} is not valid percent-encoded UTF-8"
            )));
        }
        LocalRef::OtherShape => {
            return Err(ctx.unsupported(format!(
                "{what}: $ref {reference:?} is not a local #/components/{section}/<name> \
                 reference"
            )));
        }
    };
    let target = components(root, section)
        .and_then(|map| map.get(Yaml::String(key.clone())))
        .ok_or_else(|| {
            ctx.config(format!(
                "{what}: $ref {reference:?} names a missing components.{section} entry"
            ))
        })?;
    if is_reference(target) {
        return Err(ctx.unsupported(format!(
            "{what}: $ref {reference:?} resolves to another $ref; nested references are not \
             followed"
        )));
    }
    Ok(target)
}

/// A `bindings` value that is itself a reference is refused: the D11 rule
/// needs to see the fields to decide, and components.*Bindings are not
/// followed.
fn refuse_reference_bindings(ctx: &Context, owner: &Yaml, what: &str) -> Result<(), RuntimeError> {
    if let Some(bindings) = owner.get("bindings") {
        if is_reference(bindings) {
            return Err(ctx.unsupported(format!(
                "{what}: bindings is a Reference Object, which is not resolved; inline the \
                 bindings so they can be checked"
            )));
        }
        if let Some(kafka) = bindings.get("kafka") {
            if is_reference(kafka) {
                return Err(ctx.unsupported(format!(
                    "{what}: bindings.kafka is a Reference Object, which is not resolved"
                )));
            }
        }
    }
    Ok(())
}

/// Projects `step` onto one Kafka receive target, or refuses.
pub(crate) fn project_receive_target(
    spec: &ArazzoSpec,
    step: &Step,
    retrieval_dir: Option<&Path>,
    selection: Option<&str>,
) -> Result<ReceiveTarget, RuntimeError> {
    let early = Context::before_load(step);
    // Rule 1: step form. Arazzo 1.1.0 Step Object: `action` "Indicates
    // whether the step will send (publish) or receive (subscribe)".
    match step.action {
        Some(StepAction::Receive) => {}
        Some(StepAction::Send) => {
            return Err(early.unsupported(format!(
                "step \"{}\": AsyncAPI send steps are not supported; the outgoing message \
                 contract is unresolved upstream (OAI/Arazzo-Specification#617)",
                step.step_id
            )));
        }
        None => {
            return Err(early.unsupported(format!(
                "step \"{}\": an AsyncAPI step must declare action: receive",
                step.step_id
            )));
        }
    }
    if step.correlation_id.is_some() {
        return Err(early.unsupported(format!(
            "step \"{}\": correlationId filtering is not supported in this release; remove \
             correlationId or select the message with successCriteria",
            step.step_id
        )));
    }
    if step.request_body.is_some() {
        return Err(early.unsupported(format!(
            "step \"{}\": requestBody has no meaning on a receive step",
            step.step_id
        )));
    }

    // Rule 2: target form.
    let (ctx, loaded, channel_key, channel, address) = match &step.target {
        Some(StepTarget::ChannelPath(_)) => {
            let resolved =
                resolve_local_channel(spec, step, retrieval_dir).map_err(resolve_error)?;
            let ctx = Context {
                source_name: resolved.source_name.clone(),
                document: resolved.document.clone(),
            };
            let loaded = LoadedAsyncApi {
                root: resolved.root,
                label: resolved.document,
            };
            (
                ctx,
                loaded,
                resolved.channel_key,
                resolved.channel,
                resolved.address,
            )
        }
        Some(StepTarget::OperationId(target)) => {
            resolve_operation(spec, step, target, retrieval_dir, &early)?
        }
        Some(StepTarget::OperationPath(_)) | Some(StepTarget::WorkflowId(_)) | None => {
            return Err(early.config(format!(
                "step \"{}\": an AsyncAPI receive step needs channelPath or a source-qualified \
                 operationId",
                step.step_id
            )));
        }
    };
    let root = &loaded.root;
    let channel_what = format!("channel \"{channel_key}\"");

    // Rule 3: address. AsyncAPI 3.0.0 Channel Object: "When null or absent, it
    // MUST be interpreted as unknown"; it "MAY contain Channel Address
    // Expressions", which this runtime does not evaluate.
    let topic = address.ok_or_else(|| {
        ctx.unsupported(format!(
            "{channel_what} has no address; the specification says an absent or null address \
             MUST be interpreted as unknown, so no topic can be derived"
        ))
    })?;
    if topic.is_empty() {
        return Err(ctx.unsupported(format!("{channel_what} has an empty address")));
    }
    if topic.contains('{') || topic.contains('}') || channel.get("parameters").is_some() {
        return Err(ctx.unsupported(format!(
            "{channel_what} address {topic:?} uses Channel Address Expressions / parameters, \
             which are not supported; the address must be a literal topic name"
        )));
    }

    // Rule 9 (channel part): a kafka binding `topic` that disagrees with the
    // address is refused rather than silently preferred (D11). Referenced
    // bindings cannot be checked and are refused.
    refuse_reference_bindings(&ctx, &channel, &channel_what)?;
    if let Some(bound) = channel
        .get("bindings")
        .and_then(|b| b.get("kafka"))
        .and_then(|k| k.get("topic"))
    {
        match bound.as_str() {
            Some(t) if t == topic => {}
            _ => {
                return Err(ctx.unsupported(format!(
                    "{channel_what} bindings.kafka.topic {bound:?} differs from the address \
                     {topic:?}; the address is the topic and the binding cannot override it"
                )));
            }
        }
    }

    // Rules 4–6: servers.
    let (server_key, server) = select_server(&ctx, root, &channel, &channel_key, selection)?;
    let tls_required = server.get("protocol").and_then(Yaml::as_str) == Some("kafka-secure");
    let host = server_host(&ctx, root, &server_key, server)?;

    // Rule 7: security.
    let sasl = security_mechanism(&ctx, root, &server_key, server)?;

    // Rule 8 (and the message part of rule 9) for every channel message.
    admit_messages(&ctx, root, &channel, &channel_key)?;

    Ok(ReceiveTarget {
        source_name: ctx.source_name.clone(),
        document: ctx.document.clone(),
        channel_key,
        topic,
        server_key: server_key.clone(),
        endpoint: KafkaEndpoint {
            identity: ConnectionIdentity {
                source_name: ctx.source_name,
                server_key,
                host,
            },
            tls_required,
            sasl,
        },
    })
}

type OperationResolution = (Context, LoadedAsyncApi, String, Yaml, Option<String>);

/// The `operationId: $sourceDescriptions.<name>.<operationId>` form: the
/// named Operation Object must be a direct `receive` on a local channel.
fn resolve_operation(
    spec: &ArazzoSpec,
    step: &Step,
    target: &str,
    retrieval_dir: Option<&Path>,
    early: &Context,
) -> Result<OperationResolution, RuntimeError> {
    let (source_name, operation_id) = match classify_operation_id(target) {
        OperationIdTarget::SourceQualified {
            source_name,
            operation_id,
        } => (source_name, operation_id),
        OperationIdTarget::Bare(_) => {
            return Err(early.config(format!(
                "step \"{}\": an AsyncAPI receive step's operationId must be source-qualified as \
                 $sourceDescriptions.<name>.<operationId>",
                step.step_id
            )));
        }
        OperationIdTarget::Malformed(reason) => {
            return Err(early.config(format!(
                "step \"{}\": operationId {target:?}: {reason}",
                step.step_id
            )));
        }
    };
    let source = spec
        .source_descriptions
        .iter()
        .find(|sd| sd.name == source_name)
        .ok_or_else(|| {
            early.config(format!(
                "step \"{}\": operationId names sourceDescription \"{source_name}\", which is \
                 not declared",
                step.step_id
            ))
        })?;
    if source.type_ != SourceType::AsyncApi {
        return Err(early.unsupported(format!(
            "step \"{}\": action: receive needs an asyncapi source, but sourceDescription \
             \"{source_name}\" is of type \"{}\"",
            step.step_id, source.type_
        )));
    }
    let loaded =
        load_local_asyncapi(spec, source_name, target, retrieval_dir).map_err(resolve_error)?;
    let ctx = Context {
        source_name: source_name.to_string(),
        document: loaded.label.clone(),
    };
    let what = format!("operation \"{operation_id}\"");
    let operation = loaded
        .root
        .get("operations")
        .and_then(|ops| ops.get(Yaml::String(operation_id.to_string())))
        .ok_or_else(|| ctx.config(format!("{what} is absent")))?;
    if is_reference(operation) {
        return Err(ctx.unsupported(format!(
            "{what} is a Reference Object, which is not resolved"
        )));
    }
    let fields = operation
        .as_mapping()
        .ok_or_else(|| ctx.config(format!("{what} must be a mapping")))?;
    // AsyncAPI 3.0.0 Operation Object: `action` is "Required. Use send when
    // […], and receive when the application should expect receiving
    // messages from the given channel."
    match operation.get("action").and_then(Yaml::as_str) {
        Some("receive") => {}
        Some(other) => {
            return Err(ctx.unsupported(format!(
                "{what} has action \"{other}\" but the step declares action: receive"
            )));
        }
        None => {
            return Err(ctx.config(format!("{what} has no action")));
        }
    }
    for field in ["security", "traits", "reply"] {
        if fields.contains_key(Yaml::String(field.into())) {
            return Err(ctx.unsupported(format!(
                "{what} declares `{field}`, which this runtime does not apply (operation-level \
                 security, trait merging and request-reply are unsupported); remove it or use \
                 channelPath"
            )));
        }
    }
    let channel_value = operation
        .get("channel")
        .ok_or_else(|| ctx.config(format!("{what} has no channel")))?;
    let channel_ref = ref_of(channel_value)
        .ok_or_else(|| ctx.config(format!("{what} channel must be a Reference Object")))?;
    let channel_key = match local_ref_key(channel_ref, "/channels/") {
        LocalRef::Key(key) => key,
        LocalRef::BadEncoding => {
            return Err(ctx.config(format!(
                "{what} channel $ref {channel_ref:?} is not valid percent-encoded UTF-8"
            )));
        }
        LocalRef::OtherShape => {
            return Err(ctx.unsupported(format!(
                "{what} channel $ref {channel_ref:?} is not a local #/channels/<key> reference"
            )));
        }
    };
    let resolved =
        channel_from_root(&loaded, &channel_key, target, source_name).map_err(resolve_error)?;

    // `messages`, when present, must name every channel message and nothing
    // else: without schema validation the runtime cannot tell which message
    // type a record is, so a strict subset could let an excluded type drive
    // the step.
    if let Some(list) = operation.get("messages") {
        let items = list
            .as_sequence()
            .ok_or_else(|| ctx.config(format!("{what} messages must be a list")))?;
        let prefix = format!(
            "/channels/{}/messages/",
            channel_key.replace('~', "~0").replace('/', "~1")
        );
        let channel_messages: BTreeSet<String> = resolved
            .channel
            .get("messages")
            .and_then(Yaml::as_mapping)
            .map(|m| {
                m.keys()
                    .filter_map(Yaml::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let mut listed = BTreeSet::new();
        for item in items {
            let reference = ref_of(item).ok_or_else(|| {
                ctx.config(format!("{what} messages entries must be Reference Objects"))
            })?;
            let name = match local_ref_key(reference, &prefix) {
                LocalRef::Key(name) => name,
                LocalRef::BadEncoding => {
                    return Err(ctx.config(format!(
                        "{what} messages $ref {reference:?} is not valid percent-encoded UTF-8"
                    )));
                }
                LocalRef::OtherShape => {
                    return Err(ctx.unsupported(format!(
                        "{what} messages $ref {reference:?} does not point into channel \
                         \"{channel_key}\" messages"
                    )));
                }
            };
            if !channel_messages.contains(&name) {
                return Err(ctx.config(format!(
                    "{what} messages $ref {reference:?} names a message that channel \
                     \"{channel_key}\" does not declare"
                )));
            }
            listed.insert(name);
        }
        let missing: Vec<&String> = channel_messages.difference(&listed).collect();
        if !missing.is_empty() {
            return Err(ctx.unsupported(format!(
                "{what} messages omits channel message(s) {missing:?}; received messages cannot \
                 be identified by type, so list every channel message or omit `messages`"
            )));
        }
    }

    Ok((ctx, loaded, channel_key, resolved.channel, resolved.address))
}

/// Rules 4–6. AsyncAPI 3.0.0 Channel Object `servers`: "If servers is absent
/// or empty, this channel MUST be available on all the servers defined in
/// the Servers Object." Any other shape is a document error, not a fallback.
fn select_server<'a>(
    ctx: &Context,
    root: &'a Yaml,
    channel: &Yaml,
    channel_key: &str,
    selection: Option<&str>,
) -> Result<(String, &'a Yaml), RuntimeError> {
    let what = format!("channel \"{channel_key}\"");
    let root_servers = root.get("servers").and_then(Yaml::as_mapping);
    let candidate_keys: Vec<String> = match channel.get("servers") {
        None | Some(Yaml::Null) => root_keys(root_servers),
        Some(Yaml::Sequence(list)) if list.is_empty() => root_keys(root_servers),
        Some(Yaml::Sequence(list)) => {
            let mut keys: Vec<String> = Vec::new();
            for item in list {
                let reference = ref_of(item).ok_or_else(|| {
                    ctx.config(format!("{what} servers entries must be Reference Objects"))
                })?;
                let key = match local_ref_key(reference, "/servers/") {
                    LocalRef::Key(key) => key,
                    LocalRef::BadEncoding => {
                        return Err(ctx.config(format!(
                            "{what} servers $ref {reference:?} is not valid percent-encoded UTF-8"
                        )));
                    }
                    LocalRef::OtherShape => {
                        return Err(ctx.config(format!(
                            "{what} servers $ref {reference:?} is not a local #/servers/<key> \
                             reference"
                        )));
                    }
                };
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
            keys
        }
        Some(_) => {
            return Err(ctx.config(format!(
                "{what} servers must be a list of Reference Objects (or absent/empty for all \
                 servers)"
            )));
        }
    };
    if candidate_keys.is_empty() {
        return Err(ctx.config(format!("{what} has no servers: the document declares none")));
    }
    let servers = root_servers.ok_or_else(|| ctx.config("servers must be a mapping"))?;
    let mut eligible: Vec<(String, &Yaml)> = Vec::new();
    let mut described: Vec<String> = Vec::new();
    for key in &candidate_keys {
        let server = servers
            .get(Yaml::String(key.clone()))
            .ok_or_else(|| ctx.config(format!("server \"{key}\" is absent from servers")))?;
        if is_reference(server) {
            return Err(ctx.unsupported(format!(
                "server \"{key}\" is a Reference Object, which is not resolved"
            )));
        }
        let protocol = server
            .get("protocol")
            .and_then(Yaml::as_str)
            .ok_or_else(|| ctx.config(format!("server \"{key}\" has no protocol")))?;
        described.push(format!("{key} ({protocol})"));
        if protocol == "kafka" || protocol == "kafka-secure" {
            eligible.push((key.clone(), server));
        }
    }
    if eligible.is_empty() {
        return Err(ctx.config(format!(
            "{what} has no Kafka server: candidates are {}",
            described.join(", ")
        )));
    }
    let eligible_keys = eligible
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    match selection {
        Some(selected) => eligible
            .into_iter()
            .find(|(key, _)| key == selected)
            .ok_or_else(|| {
                ctx.config(format!(
                    "--asyncapi-server selects \"{selected}\" but the eligible Kafka servers for \
                     {what} are {eligible_keys}"
                ))
            }),
        None if eligible.len() == 1 => Ok(eligible.remove(0)),
        None => Err(ctx.config(format!(
            "{what} is available on several Kafka servers ({eligible_keys}); pick one with \
             --asyncapi-server {}=<key>",
            ctx.source_name
        ))),
    }
}

fn root_keys(servers: Option<&serde_yaml_ng::Mapping>) -> Vec<String> {
    servers
        .map(|m| {
            m.keys()
                .filter_map(Yaml::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Rule 6. AsyncAPI 3.0.0 Server Object: `host` is "REQUIRED. The server host
/// name. It MAY include the port. This field supports Server Variables."
/// Only `default` values substitute (D3), and the result must contain no
/// brace at all.
fn server_host(
    ctx: &Context,
    root: &Yaml,
    server_key: &str,
    server: &Yaml,
) -> Result<String, RuntimeError> {
    let what = format!("server \"{server_key}\"");
    if is_reference(server) {
        return Err(ctx.unsupported(format!(
            "{what} is a Reference Object, which is not resolved"
        )));
    }
    let host = server
        .get("host")
        .and_then(Yaml::as_str)
        .ok_or_else(|| ctx.config(format!("{what} has no string host")))?;
    match server.get("pathname") {
        None | Some(Yaml::Null) => {}
        Some(Yaml::String(pathname)) if pathname.is_empty() => {}
        Some(Yaml::String(_)) => {
            return Err(ctx.unsupported(format!(
                "{what} declares a pathname, which has no meaning for Kafka"
            )));
        }
        Some(_) => {
            return Err(ctx.config(format!("{what} pathname must be a string")));
        }
    }
    let mut out = String::new();
    let mut rest = host;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}').ok_or_else(|| {
            ctx.config(format!("{what} host {host:?} has an unterminated variable"))
        })?;
        let name = &after[..close];
        let declared = server
            .get("variables")
            .and_then(|v| v.get(name))
            .ok_or_else(|| {
                ctx.config(format!(
                    "{what} host {host:?} uses variable {{{name}}} that is not declared under \
                     variables"
                ))
            })?;
        // Server Variable Object | Reference Object: one local hop into
        // components.serverVariables; a reference's siblings are never read.
        let variable = deref_component(
            ctx,
            root,
            declared,
            "serverVariables",
            &format!("{what} variable {{{name}}}"),
        )?;
        let default = match variable.get("default") {
            Some(Yaml::String(default)) => default.as_str(),
            Some(_) => {
                return Err(ctx.config(format!(
                    "{what} variable {{{name}}} default must be a string"
                )));
            }
            None => {
                return Err(ctx.config(format!(
                    "{what} variable {{{name}}} has no default; only defaults substitute in this \
                     release"
                )));
            }
        };
        match variable.get("enum") {
            None => {}
            Some(Yaml::Sequence(choices)) => {
                let mut allowed = Vec::with_capacity(choices.len());
                for choice in choices {
                    let Some(choice) = choice.as_str() else {
                        return Err(ctx.config(format!(
                            "{what} variable {{{name}}} enum must be a list of strings"
                        )));
                    };
                    allowed.push(choice);
                }
                if !allowed.contains(&default) {
                    return Err(ctx.config(format!(
                        "{what} variable {{{name}}} default {default:?} is not in its enum"
                    )));
                }
            }
            Some(_) => {
                return Err(ctx.config(format!(
                    "{what} variable {{{name}}} enum must be a list of strings"
                )));
            }
        }
        out.push_str(default);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    if out.is_empty() {
        return Err(ctx.config(format!("{what} host is empty")));
    }
    if out.contains('{') || out.contains('}') {
        return Err(ctx.config(format!(
            "{what} host {host:?} still contains a brace after substitution ({out:?}); variable \
             defaults must not introduce placeholders"
        )));
    }
    Ok(out)
}

/// Rule 7. AsyncAPI 3.0.0 Server Object `security`: "Only one of the security
/// scheme objects need to be satisfied". The first supported scheme is the
/// mechanism; a list with none is refused outright (D4).
fn security_mechanism(
    ctx: &Context,
    root: &Yaml,
    server_key: &str,
    server: &Yaml,
) -> Result<Option<SaslMechanism>, RuntimeError> {
    let what = format!("server \"{server_key}\" security");
    let Some(list) = server.get("security") else {
        return Ok(None);
    };
    let entries = list
        .as_sequence()
        .ok_or_else(|| ctx.config(format!("{what} must be a list")))?;
    if entries.is_empty() {
        return Ok(None);
    }
    let mut declared = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let scheme = deref_component(
            ctx,
            root,
            entry,
            "securitySchemes",
            &format!("{what}[{index}]"),
        )?;
        let type_ = scheme
            .get("type")
            .and_then(Yaml::as_str)
            .ok_or_else(|| ctx.config(format!("{what}[{index}] has no type")))?;
        let mechanism = match type_ {
            "plain" => Some(SaslMechanism::Plain),
            "scramSha256" => Some(SaslMechanism::ScramSha256),
            "scramSha512" => Some(SaslMechanism::ScramSha512),
            _ => None,
        };
        if let Some(mechanism) = mechanism {
            return Ok(Some(mechanism));
        }
        declared.push(type_.to_string());
    }
    Err(ctx.unsupported(format!(
        "{what} schemes {declared:?} are all unsupported; this runtime performs SASL plain, \
         scramSha256 and scramSha512"
    )))
}

/// Rule 8 and the message part of rule 9, for every channel message:
/// referenced bindings and schema-registry framing are refused on the
/// message and on each of its traits, whichever content type wins; the
/// effective content type — the message's own `contentType`, otherwise the
/// last trait's, otherwise the document `defaultContentType` — must be JSON.
fn admit_messages(
    ctx: &Context,
    root: &Yaml,
    channel: &Yaml,
    channel_key: &str,
) -> Result<(), RuntimeError> {
    let Some(messages) = channel.get("messages") else {
        return Ok(());
    };
    let messages = messages.as_mapping().ok_or_else(|| {
        ctx.config(format!(
            "channel \"{channel_key}\" messages must be a mapping"
        ))
    })?;
    let default = content_type_field(ctx, root, "defaultContentType", "document")?;
    for (key, value) in messages {
        let name = key.as_str().ok_or_else(|| {
            ctx.config(format!(
                "channel \"{channel_key}\" has a non-string message key"
            ))
        })?;
        let what = format!("channel \"{channel_key}\" message \"{name}\"");
        let message = deref_component(ctx, root, value, "messages", &what)?;
        if !message.is_mapping() {
            return Err(ctx.config(format!("{what} must be a mapping")));
        }
        refuse_reference_bindings(ctx, message, &what)?;
        if let Some(kafka) = message.get("bindings").and_then(|b| b.get("kafka")) {
            refuse_message_binding(ctx, &what, kafka)?;
        }

        let own = content_type_field(ctx, message, "contentType", &what)?;
        let mut from_traits: Option<&str> = None;
        if let Some(traits) = message.get("traits") {
            let traits = traits
                .as_sequence()
                .ok_or_else(|| ctx.config(format!("{what} traits must be a list")))?;
            for (index, trait_) in traits.iter().enumerate() {
                let trait_what = format!("{what} trait [{index}]");
                let trait_obj = deref_component(ctx, root, trait_, "messageTraits", &trait_what)?;
                refuse_reference_bindings(ctx, trait_obj, &trait_what)?;
                if let Some(kafka) = trait_obj.get("bindings").and_then(|b| b.get("kafka")) {
                    refuse_message_binding(ctx, &trait_what, kafka)?;
                }
                if let Some(ct) = content_type_field(ctx, trait_obj, "contentType", &trait_what)? {
                    from_traits = Some(ct);
                }
            }
        }
        let effective = own.or(from_traits).or(default);
        if let Some(ct) = effective {
            if !is_json_content_type(ct) {
                return Err(ctx.unsupported(format!(
                    "{what} has effective content type {ct:?}; only JSON (application/json or a \
                     +json suffix) is supported"
                )));
            }
        }
    }
    Ok(())
}

fn refuse_message_binding(ctx: &Context, what: &str, bindings: &Yaml) -> Result<(), RuntimeError> {
    for field in REFUSED_MESSAGE_BINDING_FIELDS {
        if bindings.get(*field).is_some() {
            return Err(ctx.unsupported(format!(
                "{what} bindings.kafka.{field} declares a schema-registry framing this runtime \
                 cannot decode"
            )));
        }
    }
    Ok(())
}

/// `contentType` (or `defaultContentType`) when present: a non-string value
/// is a document error rather than an absent field.
fn content_type_field<'a>(
    ctx: &Context,
    owner: &'a Yaml,
    field: &str,
    what: &str,
) -> Result<Option<&'a str>, RuntimeError> {
    match owner.get(field) {
        None | Some(Yaml::Null) => Ok(None),
        Some(Yaml::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(ctx.config(format!("{what} {field} must be a string"))),
    }
}

fn is_json_content_type(value: &str) -> bool {
    let media = value.split(';').next().unwrap_or("").trim();
    media.eq_ignore_ascii_case("application/json")
        || media
            .rsplit_once('+')
            .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case("json"))
}
