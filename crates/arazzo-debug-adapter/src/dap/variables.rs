use std::collections::{BTreeMap, HashMap};

use arazzo_runtime::{DebugController, DebugScopes, DebugStackFrame, StepCheckpoint};
use serde_json::{json, Value};

use super::session::{RuntimeSession, SessionState};
use super::source_index::{checkpoint_display_name, lookup_line_for_checkpoint};

const FRAME_ID_BASE: u64 = 100;

#[derive(Debug, Default)]
pub(super) struct VariableStore {
    next_ref: u64,
    entries: HashMap<u64, BTreeMap<String, Value>>,
}

impl VariableStore {
    pub(super) fn reset(&mut self) {
        self.entries.clear();
    }

    pub(super) fn insert_map(&mut self, map: BTreeMap<String, Value>) -> u64 {
        let reference = self.next_ref.max(1);
        self.next_ref = reference.saturating_add(1);
        self.entries.insert(reference, map);
        reference
    }

    pub(super) fn variables_for_reference(&mut self, reference: u64) -> Result<Vec<Value>, String> {
        let Some(entries) = self.entries.get(&reference).cloned() else {
            return Err(format!("variable reference {reference} is unavailable or expired; refresh variables at the current pause"));
        };
        let mut variables = Vec::<Value>::new();
        for (name, value) in entries {
            let child_reference = map_from_value(&value)
                .map(|map| self.insert_map(map))
                .unwrap_or(0);
            variables.push(json!({
                "name": name,
                "value": display_value(&value),
                "variablesReference": child_reference
            }));
        }
        Ok(variables)
    }
}

#[derive(Debug, Default)]
struct HttpScopeMaps {
    request: Option<BTreeMap<String, Value>>,
    response: Option<BTreeMap<String, Value>>,
}

pub(super) fn stack_trace_body(state: &SessionState) -> Result<Value, String> {
    let Some(runtime) = state.runtime.as_ref() else {
        return Err("cannot read stack: no paused workflow is available".into());
    };
    let Some(stop) = runtime.last_stop.as_ref() else {
        return Err("cannot read stack: no paused workflow is available".into());
    };
    let source_path = state
        .launch
        .as_ref()
        .map(|launch| launch.spec.clone())
        .ok_or_else(|| "cannot read stack: debug session has no source file".to_string())?;

    runtime.controller.inspect_frame(stop.depth)?;
    let stack = runtime
        .controller
        .current_stack()
        .map_err(|err| format!("cannot read stack: {err}"))?;
    if stack.is_empty() {
        return Err("cannot read stack: paused workflow has no stack frames".into());
    }
    let mut frames = Vec::new();
    for frame in stack.iter().rev() {
        let checkpoint = if frame.depth == stop.depth {
            stop.checkpoint.clone()
        } else {
            StepCheckpoint::Step
        };
        let line = lookup_line_for_checkpoint(state.source_index.as_ref(), &frame.workflow_id, &frame.step_id, &checkpoint)
            .ok_or_else(|| format!("source location unavailable in {source_path:?} for workflow: {:?}, step: {:?}, checkpoint: {}", frame.workflow_id, frame.step_id, checkpoint_display_name(&checkpoint)))?;
        let frame_id = FRAME_ID_BASE + frame.depth as u64;
        frames.push(json!({
            "id": frame_id,
            "name": format!("{}::{}", frame.workflow_id, frame.step_id),
            "line": line, "column": 1,
            "source": {"name": source_name(&source_path), "path": source_path}
        }));
    }
    Ok(json!({"totalFrames": frames.len(), "stackFrames": frames}))
}

fn selected_frame(
    runtime: &RuntimeSession,
    frame_id: Option<u64>,
) -> Result<(DebugController, DebugStackFrame), String> {
    let stop = runtime
        .last_stop
        .as_ref()
        .ok_or_else(|| "cannot inspect values: workflow has not paused".to_string())?;
    let id = frame_id.unwrap_or(FRAME_ID_BASE + stop.depth as u64);
    let depth = id
        .checked_sub(FRAME_ID_BASE)
        .and_then(|depth| usize::try_from(depth).ok())
        .ok_or_else(|| format!("invalid stack frame ID {id}"))?;
    let controller = runtime
        .controller
        .inspect_frame(depth)
        .map_err(|err| format!("cannot inspect stack frame {id}: {err}"))?;
    let frame = controller
        .current_stack()?
        .last()
        .cloned()
        .ok_or_else(|| format!("stack frame {id} is unavailable"))?;
    Ok((controller, frame))
}

pub(super) fn scopes_body(state: &mut SessionState, frame_id: u64) -> Result<Value, String> {
    let Some(runtime) = state.runtime.as_mut() else {
        return Err("cannot read scopes: debug runtime has not started".into());
    };
    let (controller, frame) = selected_frame(runtime, Some(frame_id))?;
    let scopes = controller
        .current_scopes()
        .map_err(|err| format!("cannot read scopes for frame {frame_id}: {err}"))?;
    let mut locals = scopes.locals.clone();
    let http_scopes = http_scopes_from_locals(&locals);
    locals
        .entry("workflowId".to_string())
        .or_insert(Value::String(frame.workflow_id.clone()));
    locals
        .entry("stepId".to_string())
        .or_insert(Value::String(frame.step_id.clone()));
    locals
        .entry("checkpoint".to_string())
        .or_insert(Value::String(
            runtime
                .last_stop
                .as_ref()
                .filter(|stop| stop.depth == frame.depth)
                .map(|stop| checkpoint_display_name(&stop.checkpoint))
                .unwrap_or_else(|| "step".to_string()),
        ));

    let locals_ref = runtime.variable_store.insert_map(locals);
    let mut scope_entries = vec![json!({
        "name": "Locals",
        "presentationHint": "locals",
        "variablesReference": locals_ref,
        "expensive": false
    })];

    if let Some(request_scope) = http_scopes.request {
        let request_ref = runtime.variable_store.insert_map(request_scope);
        scope_entries.push(json!({
            "name": "Request",
            "presentationHint": "registers",
            "variablesReference": request_ref,
            "expensive": false
        }));
    }
    if let Some(response_scope) = http_scopes.response {
        let response_ref = runtime.variable_store.insert_map(response_scope);
        scope_entries.push(json!({
            "name": "Response",
            "presentationHint": "registers",
            "variablesReference": response_ref,
            "expensive": false
        }));
    }

    let inputs_ref = runtime.variable_store.insert_map(scopes.inputs.clone());
    scope_entries.push(json!({
        "name": "Inputs",
        "presentationHint": "registers",
        "variablesReference": inputs_ref,
        "expensive": false
    }));

    let steps_ref = runtime
        .variable_store
        .insert_map(step_scopes_to_value_map(&scopes));
    scope_entries.push(json!({
        "name": "Steps",
        "presentationHint": "registers",
        "variablesReference": steps_ref,
        "expensive": false
    }));

    Ok(json!({ "scopes": scope_entries }))
}

pub(super) fn variables_body(state: &mut SessionState, reference: u64) -> Result<Value, String> {
    let Some(runtime) = state.runtime.as_mut() else {
        return Err("cannot read variables: debug runtime has not started".into());
    };
    let stop = runtime
        .last_stop
        .as_ref()
        .ok_or_else(|| "cannot read variables: workflow has not paused".to_string())?;
    runtime.controller.inspect_frame(stop.depth)?;
    let variables = runtime.variable_store.variables_for_reference(reference)?;
    Ok(json!({ "variables": variables }))
}

pub(super) fn evaluate_body_for_expression(
    state: &mut SessionState,
    expression: &str,
    frame_id: Option<u64>,
) -> Result<Value, String> {
    let runtime = state
        .runtime
        .as_mut()
        .ok_or_else(|| "cannot evaluate expression: debug runtime has not started".to_string())?;
    let (controller, frame) = selected_frame(runtime, frame_id)?;
    let value = evaluate_expression(runtime, &controller, &frame, expression).map_err(|error| {
        format!(
            "{error} in workflow: {:?}, step: {:?}.",
            frame.workflow_id, frame.step_id
        )
    })?;
    let child_ref = map_from_value(&value)
        .map(|map| runtime.variable_store.insert_map(map))
        .unwrap_or(0);
    Ok(json!({"result": display_value(&value), "variablesReference": child_ref}))
}

fn evaluate_expression(
    runtime: &RuntimeSession,
    controller: &DebugController,
    frame: &DebugStackFrame,
    expression: &str,
) -> Result<Value, String> {
    let trimmed = expression.trim();
    if let Some(alias) = evaluate_debug_scope_alias(controller, trimmed)? {
        return Ok(alias);
    }
    if let Some(value) = controller.try_evaluate_condition_expression(trimmed)? {
        return Ok(Value::Bool(value));
    }
    for ((workflow, step, name), output) in &runtime.outputs {
        let matches = name == trimmed
            || matches!(output, arazzo_spec::OutputValue::RuntimeExpression(value) if value == trimmed);
        if workflow == &frame.workflow_id && step == &frame.step_id && matches {
            let expression = serde_json::to_string(output)
                .map_err(|err| format!("cannot display output {name:?}: {err}"))?;
            if controller.output_is_unresolved(output)? {
                let reason = controller.evaluate_output_value(output).err();
                let detail = reason
                    .map(|reason| format!(": {reason}"))
                    .unwrap_or_default();
                return Err(format!(
                    "unresolved output value: {name:?} expression: {expression}{detail}"
                ));
            }
            return controller.evaluate_output_value(output).map_err(|err| {
                format!("cannot evaluate output value: {name:?} expression: {expression}: {err}")
            });
        }
    }
    controller.evaluate_watch_expression(trimmed)
}

fn evaluate_debug_scope_alias(
    controller: &DebugController,
    expression: &str,
) -> Result<Option<Value>, String> {
    let alias = match expression {
        "$request" => DebugScopeAlias::Request,
        "$response" => DebugScopeAlias::Response,
        _ => return Ok(None),
    };
    let scopes = controller.current_scopes()?;
    let http_scopes = http_scopes_from_locals(&scopes.locals);
    let map = match alias {
        DebugScopeAlias::Request => http_scopes.request,
        DebugScopeAlias::Response => http_scopes.response,
    };
    let mut map = map.ok_or_else(|| format!("{expression} is unavailable at this checkpoint"))?;
    // HTTP metadata is valid even when the message has no parsed body.
    // Only add a body when this checkpoint captured one.
    let body_expr = match alias {
        DebugScopeAlias::Request => "$request.body",
        DebugScopeAlias::Response => "$response.body",
    };
    let has_body = match alias {
        DebugScopeAlias::Request => scopes.locals.contains_key("requestBody"),
        DebugScopeAlias::Response => scopes.locals.contains_key("responseBodyRaw"),
    };
    if has_body {
        map.insert(
            "body".into(),
            controller.evaluate_watch_expression(body_expr)?,
        );
    }
    Ok(Some(Value::Object(map.into_iter().collect())))
}

#[derive(Clone, Copy)]
enum DebugScopeAlias {
    Request,
    Response,
}

fn http_scopes_from_locals(locals: &BTreeMap<String, Value>) -> HttpScopeMaps {
    let mut request = BTreeMap::<String, Value>::new();
    insert_scope_value(&mut request, "method", locals, "requestMethod");
    insert_scope_value(&mut request, "url", locals, "requestUrl");
    insert_scope_value(&mut request, "headers", locals, "requestHeaders");
    insert_scope_value(&mut request, "body", locals, "requestBody");

    let mut response = BTreeMap::<String, Value>::new();
    insert_scope_value(&mut response, "statusCode", locals, "responseStatusCode");
    insert_scope_value(&mut response, "contentType", locals, "responseContentType");
    insert_scope_value(&mut response, "headers", locals, "responseHeaders");
    insert_scope_value(&mut response, "bodyPreview", locals, "responseBodyPreview");
    if locals.contains_key("responseBodyRaw") {
        response.insert("bodyRawAvailable".to_string(), Value::Bool(true));
    }

    HttpScopeMaps {
        request: (!request.is_empty()).then_some(request),
        response: (!response.is_empty()).then_some(response),
    }
}

fn insert_scope_value(
    target: &mut BTreeMap<String, Value>,
    target_key: &str,
    source: &BTreeMap<String, Value>,
    source_key: &str,
) {
    if let Some(value) = source.get(source_key) {
        target.insert(target_key.to_string(), value.clone());
    }
}

fn step_scopes_to_value_map(scopes: &DebugScopes) -> BTreeMap<String, Value> {
    let mut map = BTreeMap::new();
    for (step_id, outputs) in &scopes.steps {
        let mut object = serde_json::Map::new();
        for (name, value) in outputs {
            object.insert(name.clone(), value.clone());
        }
        map.insert(step_id.clone(), Value::Object(object));
    }
    map
}

fn map_from_value(value: &Value) -> Option<BTreeMap<String, Value>> {
    match value {
        Value::Object(object) => {
            let mut map = BTreeMap::new();
            for (key, value) in object {
                map.insert(key.clone(), value.clone());
            }
            Some(map)
        }
        Value::Array(array) => {
            let mut map = BTreeMap::new();
            for (index, value) in array.iter().enumerate() {
                map.insert(format!("[{index}]"), value.clone());
            }
            Some(map)
        }
        _ => None,
    }
}

fn display_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Array(_) | Value::Object(_) => match serde_json::to_string(value) {
            Ok(serialized) => serialized,
            Err(_) => "<unprintable>".to_string(),
        },
    }
}

fn source_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workflow")
        .to_string()
}

#[cfg(test)]
mod inspection_tests {
    use super::super::session::{
        cleanup_runtime, ensure_runtime_started, EngineEvent, LaunchConfig,
    };
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn expired_variable_handles_do_not_alias_new_values() {
        let mut store = VariableStore::default();
        let first = store.insert_map(BTreeMap::from([("old".into(), json!(1))]));
        store.reset();
        let second = store.insert_map(BTreeMap::from([("new".into(), json!(2))]));
        assert_ne!(first, second);
        assert!(store
            .variables_for_reference(first)
            .err()
            .unwrap_or_else(|| panic!("expected expired reference error"))
            .contains("expired"));
        assert_eq!(
            store
                .variables_for_reference(second)
                .unwrap_or_else(|err| panic!("reading variables: {err}"))[0]["name"],
            "new"
        );
    }

    #[test]
    fn missing_source_mapping_is_an_error_not_line_one() {
        let mut state = SessionState {
            launch: Some(LaunchConfig {
                spec: format!(
                    "{}/../../testdata/debugger-diagnostics/frames.arazzo.yaml",
                    env!("CARGO_MANIFEST_DIR")
                ),
                workflow_id: Some("parent".into()),
                inputs: BTreeMap::from([("label".into(), json!("parent-value"))]),
                dry_run: true,
                stop_on_entry: true,
            }),
            ..Default::default()
        };
        let (tx, rx) = mpsc::channel();
        ensure_runtime_started(&mut state, &tx)
            .unwrap_or_else(|err| panic!("starting fixture: {err}"));
        let event = rx.recv_timeout(Duration::from_secs(2));
        if let Ok(EngineEvent::Stopped(stop)) = event {
            state
                .runtime
                .as_mut()
                .unwrap_or_else(|| panic!("missing runtime"))
                .last_stop = Some(stop);
        }
        let result = stack_trace_body(&state);
        cleanup_runtime(&mut state);
        let message = result
            .err()
            .unwrap_or_else(|| panic!("missing index cannot produce a source line"));
        assert!(message.contains("source location unavailable"), "{message}");
        assert!(message.contains("parent") && message.contains("call-child"));
    }
}
