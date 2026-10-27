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

use std::path::Path;

use arazzo_spec::{
    classify_operation_id, ArazzoSpec, OperationIdTarget, SourceType, Step, StepAction, StepTarget,
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

/// Binding fields the D11 rule ignores, listed in the module docs for the
/// user guide. Everything else under a `kafka` binding is either refused
/// (see [`REFUSED_MESSAGE_BINDING_FIELDS`] and the channel `topic` rule) or
/// absent from the specification we vendor.
pub(crate) const IGNORED_BINDING_FIELDS: &[&str] = &[
    "server bindings (all)",
    "channel kafka.partitions",
    "channel kafka.replicas",
    "channel kafka.topicConfiguration",
    "operation kafka.groupId",
    "operation kafka.clientId",
    "message kafka.key",
    "non-kafka bindings (all)",
];

/// Message binding fields that change what is on the wire and are refused.
const REFUSED_MESSAGE_BINDING_FIELDS: &[&str] = &[
    "schemaIdLocation",
    "schemaIdPayloadEncoding",
    "schemaLookupStrategy",
];

struct Context {
    source_name: String,
    document: String,
}

impl Context {
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

/// Local `#/a/b/<key>` reference: returns the unescaped key when `reference`
/// is exactly `prefix` followed by one more token.
fn local_ref_key(reference: &str, prefix: &str) -> Option<String> {
    let rest = reference.strip_prefix(prefix)?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(rest.replace("~1", "/").replace("~0", "~"))
}

fn ref_of(value: &Yaml) -> Option<&str> {
    let map = value.as_mapping()?;
    if map.len() != 1 {
        return None;
    }
    map.get(Yaml::String("$ref".into()))?.as_str()
}

fn components<'a>(root: &'a Yaml, section: &str) -> Option<&'a Yaml> {
    root.get("components")?.get(section)
}

/// Follows one local `$ref` into `components.<section>`; nested refs are
/// refused. Returns the object and the name it was found under.
fn deref_component<'a>(
    ctx: &Context,
    root: &'a Yaml,
    value: &'a Yaml,
    section: &str,
    what: &str,
) -> Result<(&'a Yaml, Option<String>), RuntimeError> {
    let Some(reference) = ref_of(value) else {
        return Ok((value, None));
    };
    let prefix = format!("#/components/{section}/");
    let key = local_ref_key(reference, &prefix).ok_or_else(|| {
        ctx.unsupported(format!(
            "{what} $ref {reference:?} is not a local #/components/{section}/<name> reference"
        ))
    })?;
    let target = components(root, section)
        .and_then(|map| map.get(Yaml::String(key.clone())))
        .ok_or_else(|| {
            ctx.config(format!(
                "{what} $ref {reference:?} names a missing components.{section} entry"
            ))
        })?;
    if ref_of(target).is_some() {
        return Err(ctx.unsupported(format!(
            "{what} $ref {reference:?} resolves to another $ref; nested references are not followed"
        )));
    }
    Ok((target, Some(key)))
}

/// Projects `step` onto one Kafka receive target, or refuses.
pub(crate) fn project_receive_target(
    spec: &ArazzoSpec,
    step: &Step,
    retrieval_dir: Option<&Path>,
    selection: Option<&str>,
) -> Result<ReceiveTarget, RuntimeError> {
    // Rule 1: step form. Arazzo 1.1.0 Step Object: `action` "Indicates
    // whether the step will send (publish) or receive (subscribe)".
    match step.action {
        Some(StepAction::Receive) => {}
        Some(StepAction::Send) => {
            return Err(RuntimeError::new(
                RuntimeErrorKind::UnsupportedAsyncApiTransport,
                format!(
                    "step \"{}\": AsyncAPI send steps are not supported; the outgoing message \
                     contract is unresolved upstream (OAI/Arazzo-Specification#617)",
                    step.step_id
                ),
            ));
        }
        None => {
            return Err(RuntimeError::new(
                RuntimeErrorKind::UnsupportedAsyncApiTransport,
                format!(
                    "step \"{}\": an AsyncAPI step must declare action: receive",
                    step.step_id
                ),
            ));
        }
    }
    if step.correlation_id.is_some() {
        return Err(RuntimeError::new(
            RuntimeErrorKind::UnsupportedAsyncApiTransport,
            format!(
                "step \"{}\": correlationId filtering is not supported in this release; \
                 remove correlationId or select the message with successCriteria",
                step.step_id
            ),
        ));
    }
    if step.request_body.is_some() {
        return Err(RuntimeError::new(
            RuntimeErrorKind::UnsupportedAsyncApiTransport,
            format!(
                "step \"{}\": requestBody has no meaning on a receive step",
                step.step_id
            ),
        ));
    }

    // Rule 2: target form.
    let (ctx, loaded, channel_key, channel, address, admitted_messages) = match &step.target {
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
                None,
            )
        }
        Some(StepTarget::OperationId(target)) => {
            resolve_operation(spec, step, target, retrieval_dir)?
        }
        Some(StepTarget::OperationPath(_)) | Some(StepTarget::WorkflowId(_)) | None => {
            return Err(RuntimeError::new(
                RuntimeErrorKind::MessageTransportConfig,
                format!(
                    "step \"{}\": an AsyncAPI receive step needs channelPath or a \
                     source-qualified operationId",
                    step.step_id
                ),
            ));
        }
    };
    let root = &loaded.root;

    // Rule 3: address. AsyncAPI 3.0.0 Channel Object: "When null or absent, it
    // MUST be interpreted as unknown"; it "MAY contain Channel Address
    // Expressions", which this runtime does not evaluate.
    let topic = address.ok_or_else(|| {
        ctx.unsupported(format!(
            "channel \"{channel_key}\" has no address; the specification says an absent or \
             null address MUST be interpreted as unknown, so no topic can be derived"
        ))
    })?;
    if topic.is_empty() {
        return Err(ctx.unsupported(format!("channel \"{channel_key}\" has an empty address")));
    }
    if topic.contains('{') || topic.contains('}') || channel.get("parameters").is_some() {
        return Err(ctx.unsupported(format!(
            "channel \"{channel_key}\" address {topic:?} uses Channel Address Expressions / \
             parameters, which are not supported; the address must be a literal topic name"
        )));
    }

    // Rule 9 (channel part): a kafka binding `topic` that disagrees with the
    // address is refused rather than silently preferred (D11).
    if let Some(bound) = channel
        .get("bindings")
        .and_then(|b| b.get("kafka"))
        .and_then(|k| k.get("topic"))
    {
        match bound.as_str() {
            Some(t) if t == topic => {}
            _ => {
                return Err(ctx.unsupported(format!(
                    "channel \"{channel_key}\" bindings.kafka.topic {bound:?} differs from the \
                     address {topic:?}; the address is the topic and the binding cannot override it"
                )));
            }
        }
    }

    // Rules 4–6: servers.
    let (server_key, server) = select_server(&ctx, root, &channel, &channel_key, selection)?;
    let tls_required = server.get("protocol").and_then(Yaml::as_str) == Some("kafka-secure");
    let host = server_host(&ctx, &server_key, server)?;

    // Rule 7: security.
    let sasl = security_mechanism(&ctx, root, &server_key, server)?;

    // Rule 8: content types for every admitted message.
    admit_content_types(
        &ctx,
        root,
        &channel,
        &channel_key,
        admitted_messages.as_deref(),
    )?;

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

type OperationResolution = (
    Context,
    LoadedAsyncApi,
    String,
    Yaml,
    Option<String>,
    Option<Vec<String>>,
);

/// The `operationId: $sourceDescriptions.<name>.<operationId>` form: the
/// named Operation Object must be a direct `receive` on a local channel.
fn resolve_operation(
    spec: &ArazzoSpec,
    step: &Step,
    target: &str,
    retrieval_dir: Option<&Path>,
) -> Result<OperationResolution, RuntimeError> {
    let (source_name, operation_id) = match classify_operation_id(target) {
        OperationIdTarget::SourceQualified {
            source_name,
            operation_id,
        } => (source_name, operation_id),
        OperationIdTarget::Bare(_) => {
            return Err(RuntimeError::new(
                RuntimeErrorKind::MessageTransportConfig,
                format!(
                    "step \"{}\": an AsyncAPI receive step's operationId must be source-qualified \
                     as $sourceDescriptions.<name>.<operationId>",
                    step.step_id
                ),
            ));
        }
        OperationIdTarget::Malformed(reason) => {
            return Err(RuntimeError::new(
                RuntimeErrorKind::MessageTransportConfig,
                format!(
                    "step \"{}\": operationId {target:?}: {reason}",
                    step.step_id
                ),
            ));
        }
    };
    let source = spec
        .source_descriptions
        .iter()
        .find(|sd| sd.name == source_name)
        .ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorKind::MessageTransportConfig,
                format!(
                    "step \"{}\": operationId names sourceDescription \"{source_name}\", which \
                     is not declared",
                    step.step_id
                ),
            )
        })?;
    if source.type_ != SourceType::AsyncApi {
        return Err(RuntimeError::new(
            RuntimeErrorKind::UnsupportedAsyncApiTransport,
            format!(
                "step \"{}\": action: receive needs an asyncapi source, but sourceDescription \
                 \"{source_name}\" is of type \"{}\"",
                step.step_id, source.type_
            ),
        ));
    }
    let loaded =
        load_local_asyncapi(spec, source_name, target, retrieval_dir).map_err(resolve_error)?;
    let ctx = Context {
        source_name: source_name.to_string(),
        document: loaded.label.clone(),
    };
    let operation = loaded
        .root
        .get("operations")
        .and_then(|ops| ops.get(Yaml::String(operation_id.to_string())))
        .ok_or_else(|| ctx.config(format!("operation \"{operation_id}\" is absent")))?;
    if ref_of(operation).is_some() {
        return Err(ctx.unsupported(format!(
            "operation \"{operation_id}\" is a Reference Object, which is not resolved"
        )));
    }
    let fields = operation
        .as_mapping()
        .ok_or_else(|| ctx.config(format!("operation \"{operation_id}\" must be a mapping")))?;
    // AsyncAPI 3.0.0 Operation Object: `action` is "Required. Use send when
    // […], and receive when the application should expect receiving
    // messages from the given channel."
    match operation.get("action").and_then(Yaml::as_str) {
        Some("receive") => {}
        Some(other) => {
            return Err(ctx.unsupported(format!(
                "operation \"{operation_id}\" has action \"{other}\" but the step declares \
                 action: receive"
            )));
        }
        None => {
            return Err(ctx.config(format!("operation \"{operation_id}\" has no action")));
        }
    }
    for field in ["security", "traits", "reply"] {
        if fields.contains_key(Yaml::String(field.into())) {
            return Err(ctx.unsupported(format!(
                "operation \"{operation_id}\" declares `{field}`, which this runtime does not \
                 apply (operation-level security, trait merging and request-reply are \
                 unsupported); remove it or use channelPath"
            )));
        }
    }
    let channel_ref = operation.get("channel").and_then(ref_of).ok_or_else(|| {
        ctx.config(format!(
            "operation \"{operation_id}\" channel must be a Reference Object"
        ))
    })?;
    let channel_key = local_ref_key(channel_ref, "#/channels/").ok_or_else(|| {
        ctx.unsupported(format!(
            "operation \"{operation_id}\" channel $ref {channel_ref:?} is not a local \
             #/channels/<key> reference"
        ))
    })?;
    let resolved =
        channel_from_root(&loaded, &channel_key, target, source_name).map_err(resolve_error)?;

    // `messages`, when present, must name every channel message: without
    // schema validation the runtime cannot tell which message type a record
    // is, so a strict subset could let an excluded type drive the step.
    let admitted = match operation.get("messages") {
        None => None,
        Some(list) => {
            let items = list.as_sequence().ok_or_else(|| {
                ctx.config(format!(
                    "operation \"{operation_id}\" messages must be a list"
                ))
            })?;
            let prefix = format!(
                "#/channels/{}/messages/",
                channel_key.replace('~', "~0").replace('/', "~1")
            );
            let mut names = Vec::new();
            for item in items {
                let reference = ref_of(item).ok_or_else(|| {
                    ctx.config(format!(
                        "operation \"{operation_id}\" messages entries must be Reference Objects"
                    ))
                })?;
                let name = local_ref_key(reference, &prefix).ok_or_else(|| {
                    ctx.unsupported(format!(
                        "operation \"{operation_id}\" messages $ref {reference:?} does not point \
                         into channel \"{channel_key}\" messages"
                    ))
                })?;
                names.push(name);
            }
            let channel_messages: Vec<String> = resolved
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
            let missing: Vec<&String> = channel_messages
                .iter()
                .filter(|name| !names.contains(name))
                .collect();
            if !missing.is_empty() {
                return Err(ctx.unsupported(format!(
                    "operation \"{operation_id}\" messages omits channel message(s) {missing:?}; \
                     received messages cannot be identified by type, so list every channel \
                     message or omit `messages`"
                )));
            }
            Some(names)
        }
    };

    Ok((
        ctx,
        loaded,
        channel_key,
        resolved.channel,
        resolved.address,
        admitted,
    ))
}

/// Rules 4–6. AsyncAPI 3.0.0 Channel Object `servers`: "If servers is absent
/// or empty, this channel MUST be available on all the servers defined in
/// the Servers Object."
fn select_server<'a>(
    ctx: &Context,
    root: &'a Yaml,
    channel: &Yaml,
    channel_key: &str,
    selection: Option<&str>,
) -> Result<(String, &'a Yaml), RuntimeError> {
    let servers = root.get("servers").and_then(Yaml::as_mapping);
    let candidate_keys: Vec<String> = match channel.get("servers") {
        Some(list) if !list.as_sequence().is_none_or(|s| s.is_empty()) => {
            let mut keys = Vec::new();
            for item in list.as_sequence().into_iter().flatten() {
                let reference = ref_of(item).ok_or_else(|| {
                    ctx.config(format!(
                        "channel \"{channel_key}\" servers entries must be Reference Objects"
                    ))
                })?;
                let key = local_ref_key(reference, "#/servers/").ok_or_else(|| {
                    ctx.config(format!(
                        "channel \"{channel_key}\" servers $ref {reference:?} is not a local \
                         #/servers/<key> reference"
                    ))
                })?;
                keys.push(key);
            }
            keys
        }
        _ => servers
            .map(|m| {
                m.keys()
                    .filter_map(Yaml::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    };
    if candidate_keys.is_empty() {
        return Err(ctx.config(format!(
            "channel \"{channel_key}\" has no servers: the document declares none"
        )));
    }
    let servers = servers.ok_or_else(|| ctx.config("servers must be a mapping"))?;
    let mut eligible: Vec<(String, &Yaml)> = Vec::new();
    let mut described: Vec<String> = Vec::new();
    for key in &candidate_keys {
        let server = servers
            .get(Yaml::String(key.clone()))
            .ok_or_else(|| ctx.config(format!("server \"{key}\" is absent from servers")))?;
        if ref_of(server).is_some() {
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
            "channel \"{channel_key}\" has no Kafka server: candidates are {}",
            described.join(", ")
        )));
    }
    match selection {
        Some(selected) => eligible
            .into_iter()
            .find(|(key, _)| key == selected)
            .ok_or_else(|| {
                ctx.config(format!(
                    "--asyncapi-server selects \"{selected}\" but the eligible Kafka servers for \
                     channel \"{channel_key}\" are {}",
                    described.join(", ")
                ))
            }),
        None if eligible.len() == 1 => Ok(eligible.remove(0)),
        None => Err(ctx.config(format!(
            "channel \"{channel_key}\" is available on several Kafka servers ({}); pick one with \
             --asyncapi-server {}=<key>",
            eligible
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            ctx.source_name
        ))),
    }
}

/// Rule 6. AsyncAPI 3.0.0 Server Object: `host` is "REQUIRED. The server host
/// name. It MAY include the port. This field supports Server Variables."
/// Only `default` values substitute (D3).
fn server_host(ctx: &Context, server_key: &str, server: &Yaml) -> Result<String, RuntimeError> {
    let host = server
        .get("host")
        .and_then(Yaml::as_str)
        .ok_or_else(|| ctx.config(format!("server \"{server_key}\" has no string host")))?;
    if server
        .get("pathname")
        .and_then(Yaml::as_str)
        .is_some_and(|p| !p.is_empty())
    {
        return Err(ctx.unsupported(format!(
            "server \"{server_key}\" declares a pathname, which has no meaning for Kafka"
        )));
    }
    let mut out = String::new();
    let mut rest = host;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}').ok_or_else(|| {
            ctx.config(format!(
                "server \"{server_key}\" host {host:?} has an unterminated variable"
            ))
        })?;
        let name = &after[..close];
        let variable = server
            .get("variables")
            .and_then(|v| v.get(name))
            .ok_or_else(|| {
                ctx.config(format!(
                    "server \"{server_key}\" host {host:?} uses variable {{{name}}} that is not \
                     declared under variables"
                ))
            })?;
        let default = variable
            .get("default")
            .and_then(Yaml::as_str)
            .ok_or_else(|| {
                ctx.config(format!(
                    "server \"{server_key}\" variable {{{name}}} has no string default; only \
                     defaults substitute in this release"
                ))
            })?;
        if let Some(choices) = variable.get("enum").and_then(Yaml::as_sequence) {
            if !choices.iter().any(|c| c.as_str() == Some(default)) {
                return Err(ctx.config(format!(
                    "server \"{server_key}\" variable {{{name}}} default {default:?} is not in \
                     its enum"
                )));
            }
        }
        out.push_str(default);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    if out.is_empty() {
        return Err(ctx.config(format!("server \"{server_key}\" host is empty")));
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
    let Some(list) = server.get("security") else {
        return Ok(None);
    };
    let entries = list
        .as_sequence()
        .ok_or_else(|| ctx.config(format!("server \"{server_key}\" security must be a list")))?;
    if entries.is_empty() {
        return Ok(None);
    }
    let mut declared = Vec::new();
    for entry in entries {
        let (scheme, _) = deref_component(ctx, root, entry, "securitySchemes", "security scheme")?;
        let type_ = scheme.get("type").and_then(Yaml::as_str).ok_or_else(|| {
            ctx.config(format!(
                "server \"{server_key}\" has a security scheme without a type"
            ))
        })?;
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
        "server \"{server_key}\" security schemes {declared:?} are all unsupported; this runtime \
         performs SASL plain, scramSha256 and scramSha512"
    )))
}

/// Rule 8. Effective content type with AsyncAPI precedence — the message's
/// own `contentType`, otherwise the last trait's, otherwise the document
/// `defaultContentType` — must be JSON.
fn admit_content_types(
    ctx: &Context,
    root: &Yaml,
    channel: &Yaml,
    channel_key: &str,
    admitted: Option<&[String]>,
) -> Result<(), RuntimeError> {
    let Some(messages) = channel.get("messages") else {
        return Ok(());
    };
    let messages = messages.as_mapping().ok_or_else(|| {
        ctx.config(format!(
            "channel \"{channel_key}\" messages must be a mapping"
        ))
    })?;
    let default = root.get("defaultContentType").and_then(Yaml::as_str);
    for (key, value) in messages {
        let name = key.as_str().ok_or_else(|| {
            ctx.config(format!(
                "channel \"{channel_key}\" has a non-string message key"
            ))
        })?;
        if admitted.is_some_and(|list| !list.iter().any(|n| n == name)) {
            continue;
        }
        let (message, _) = deref_component(ctx, root, value, "messages", "message")?;
        let message_map = message.as_mapping().ok_or_else(|| {
            ctx.config(format!(
                "channel \"{channel_key}\" message \"{name}\" must be a mapping"
            ))
        })?;
        if let Some(bindings) = message_map
            .get(Yaml::String("bindings".into()))
            .and_then(|b| b.get("kafka"))
        {
            refuse_message_binding(ctx, channel_key, name, bindings)?;
        }
        let mut effective = message_map
            .get(Yaml::String("contentType".into()))
            .and_then(Yaml::as_str);
        if effective.is_none() {
            if let Some(traits) = message_map.get(Yaml::String("traits".into())) {
                let traits = traits.as_sequence().ok_or_else(|| {
                    ctx.config(format!(
                        "channel \"{channel_key}\" message \"{name}\" traits must be a list"
                    ))
                })?;
                for trait_ in traits {
                    let (trait_obj, _) =
                        deref_component(ctx, root, trait_, "messageTraits", "message trait")?;
                    if let Some(bindings) = trait_obj.get("bindings").and_then(|b| b.get("kafka")) {
                        refuse_message_binding(ctx, channel_key, name, bindings)?;
                    }
                    if let Some(ct) = trait_obj.get("contentType").and_then(Yaml::as_str) {
                        effective = Some(ct);
                    }
                }
            }
        }
        let effective = effective.or(default);
        if let Some(ct) = effective {
            if !is_json_content_type(ct) {
                return Err(ctx.unsupported(format!(
                    "channel \"{channel_key}\" message \"{name}\" has effective content type \
                     {ct:?}; only JSON (application/json or a +json suffix) is supported"
                )));
            }
        }
    }
    Ok(())
}

fn refuse_message_binding(
    ctx: &Context,
    channel_key: &str,
    name: &str,
    bindings: &Yaml,
) -> Result<(), RuntimeError> {
    for field in REFUSED_MESSAGE_BINDING_FIELDS {
        if bindings.get(*field).is_some() {
            return Err(ctx.unsupported(format!(
                "channel \"{channel_key}\" message \"{name}\" bindings.kafka.{field} declares a \
                 schema-registry framing this runtime cannot decode"
            )));
        }
    }
    Ok(())
}

fn is_json_content_type(value: &str) -> bool {
    let media = value.split(';').next().unwrap_or("").trim();
    media.eq_ignore_ascii_case("application/json")
        || media
            .rsplit_once('+')
            .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case("json"))
}
