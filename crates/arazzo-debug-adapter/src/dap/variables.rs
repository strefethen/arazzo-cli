use std::collections::{BTreeMap, HashMap};

use arazzo_runtime::{DebugScopes, StepCheckpoint};
use serde_json::{json, Value};

use super::responses::evaluate_body;
use super::session::{RuntimeSession, SessionState};
use super::source_index::{
    checkpoint_display_name, lookup_line_for_checkpoint, lookup_output_expression, SourceIndex,
};

const FRAME_ID_BASE: u64 = 100;

#[derive(Debug, Default)]
pub(super) struct VariableStore {
    next_ref: u64,
    entries: HashMap<u64, BTreeMap<String, Value>>,
}

impl VariableStore {
    pub(super) fn reset(&mut self) {
        self.next_ref = 1;
        self.entries.clear();
    }

    pub(super) fn insert_map(&mut self, map: BTreeMap<String, Value>) -> u64 {
        let reference = self.next_ref.max(1);
        self.next_ref = reference.saturating_add(1);
        self.entries.insert(reference, map);
        reference
    }

    pub(super) fn variables_for_reference(&mut self, reference: u64) -> Vec<Value> {
        let Some(entries) = self.entries.get(&reference).cloned() else {
            return Vec::new();
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
        variables
    }
}

#[derive(Debug, Default)]
struct HttpScopeMaps {
    request: Option<BTreeMap<String, Value>>,
    response: Option<BTreeMap<String, Value>>,
}

pub(super) fn stack_trace_body(state: &SessionState) -> Value {
    let Some(runtime) = state.runtime.as_ref() else {
        return json!({ "stackFrames": [], "totalFrames": 0 });
    };
    let Some(stop) = runtime.last_stop.as_ref() else {
        return json!({ "stackFrames": [], "totalFrames": 0 });
    };
    let source_path = state
        .launch
        .as_ref()
        .map(|launch| launch.spec.clone())
        .unwrap_or_default();

    let stack = runtime.controller.current_stack().unwrap_or_default();
    let mut frames = Vec::<Value>::new();
    if stack.is_empty() {
        let line = lookup_line_for_checkpoint(
            state.source_index.as_ref(),
            &stop.workflow_id,
            &stop.step_id,
            &stop.checkpoint,
        )
        .unwrap_or(1);
        frames.push(json!({
            "id": FRAME_ID_BASE,
            "name": format!("{}::{}", stop.workflow_id, stop.step_id),
            "line": line,
            "column": 1,
            "source": {
                "name": source_name(&source_path),
                "path": source_path
            }
        }));
    } else {
        for frame in stack.iter().rev() {
            let checkpoint = if frame.depth == stop.depth {
                stop.checkpoint.clone()
            } else {
                StepCheckpoint::Step
            };
            let line = lookup_line_for_checkpoint(
                state.source_index.as_ref(),
                &frame.workflow_id,
                &frame.step_id,
                &checkpoint,
            )
            .unwrap_or(1);
            let frame_id = FRAME_ID_BASE.saturating_add(u64::try_from(frame.depth).unwrap_or(0));
            frames.push(json!({
                "id": frame_id,
                "name": format!("{}::{}", frame.workflow_id, frame.step_id),
                "line": line,
                "column": 1,
                "source": {
                    "name": source_name(&source_path),
                    "path": source_path
                }
            }));
        }
    }

    json!({
        "stackFrames": frames,
        "totalFrames": frames.len()
    })
}

pub(super) fn scopes_body(state: &mut SessionState) -> Value {
    let Some(runtime) = state.runtime.as_mut() else {
        return json!({ "scopes": [] });
    };
    let Some(stop) = runtime.last_stop.as_ref() else {
        return json!({ "scopes": [] });
    };
    let scopes = runtime.controller.current_scopes().unwrap_or_default();
    runtime.variable_store.reset();

    let mut locals = scopes.locals.clone();
    let http_scopes = http_scopes_from_locals(&locals);
    locals
        .entry("workflowId".to_string())
        .or_insert(Value::String(stop.workflow_id.clone()));
    locals
        .entry("stepId".to_string())
        .or_insert(Value::String(stop.step_id.clone()));
    locals
        .entry("checkpoint".to_string())
        .or_insert(Value::String(checkpoint_display_name(&stop.checkpoint)));

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

    json!({ "scopes": scope_entries })
}

pub(super) fn variables_body(state: &mut SessionState, reference: u64) -> Value {
    let Some(runtime) = state.runtime.as_mut() else {
        return json!({ "variables": [] });
    };
    let variables = runtime.variable_store.variables_for_reference(reference);
    json!({ "variables": variables })
}

pub(super) fn evaluate_body_for_expression(state: &mut SessionState, expression: &str) -> Value {
    let source_index = state.source_index.clone();
    let Some(runtime) = state.runtime.as_mut() else {
        return evaluate_body("runtime not started".to_string());
    };

    let value = evaluate_expression_with_fallback(runtime, source_index.as_ref(), expression)
        .unwrap_or_else(|| Value::String("null".to_string()));
    let child_ref = map_from_value(&value)
        .map(|map| runtime.variable_store.insert_map(map))
        .unwrap_or(0);
    json!({
        "result": display_value(&value),
        "variablesReference": child_ref
    })
}

fn evaluate_expression_with_fallback(
    runtime: &RuntimeSession,
    source_index: Option<&SourceIndex>,
    expression: &str,
) -> Option<Value> {
    let trimmed = expression.trim();
    if !trimmed.is_empty() && !trimmed.starts_with('$') && !trimmed.starts_with('/') {
        if let Some(stop) = runtime.last_stop.as_ref() {
            if let Some(mapped) =
                lookup_output_expression(source_index, &stop.workflow_id, &stop.step_id, trimmed)
            {
                return try_evaluate_watch_expression(runtime, mapped);
            }
        }
    }

    try_evaluate_watch_expression(runtime, trimmed)
}

fn try_evaluate_watch_expression(runtime: &RuntimeSession, expression: &str) -> Option<Value> {
    // Intentional: watch/evaluate should degrade to "null" rather than hard-fail DAP.
    runtime
        .controller
        .evaluate_watch_expression(expression)
        .ok()
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
