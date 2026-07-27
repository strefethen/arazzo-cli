//! OpenAPI → Arazzo CRUD workflow generator.
//!
//! Given an OpenAPI 3.0 spec, produces a runnable Arazzo 1.0 document with
//! CRUD workflows, chained steps, authentication setup, and realistic request
//! bodies derived from schema examples.

use std::collections::{BTreeMap, HashSet};

use arazzo_spec::{
    ActionType, ArazzoSpec, Info, JsonSchemaType, OnAction, ParamLocation, Parameter, PropertyDef,
    RequestBody, SchemaObject, SourceDescription, SourceType, Step, StepTarget, SuccessCriterion,
    Workflow,
};
use openapiv3::{OpenAPI, ReferenceOr, StatusCode};

use crate::examples::{generate_example, json_to_yml};
use crate::refs::{resolve_response_ref, resolve_schema_ref};

// ─── Public API ──────────────────────────────────────────────────────────────

/// Result of the generation process.
pub struct GenerateOutput {
    pub spec: ArazzoSpec,
    pub warnings: Vec<String>,
    pub resources: Vec<String>,
    pub auth_type: Option<String>,
}

/// Generate CRUD workflows from an OpenAPI spec.
pub fn generate_crud(openapi: &OpenAPI, spec_filename: &str) -> Result<GenerateOutput, String> {
    let mut warnings = Vec::new();

    check_openapi_version(&openapi.openapi, &mut warnings)?;
    let server_url = extract_server_url(openapi, &mut warnings)?;
    let groups = group_resources(openapi, &mut warnings);
    if groups.is_empty() {
        return Err("no CRUD resource groups found in the OpenAPI spec".to_string());
    }

    let auth = detect_auth(openapi);
    let source_name = derive_source_name(&openapi.info.title, spec_filename);

    let mut workflows = Vec::new();
    let resource_names: Vec<String> = groups.iter().map(|g| g.name.clone()).collect();

    for group in &groups {
        workflows.push(build_workflow(group, &source_name, &auth, openapi));
    }

    let spec = ArazzoSpec {
        arazzo: "1.0.0".to_string(),
        info: Info {
            title: format!("{} Workflows", openapi.info.title),
            version: "1.0.0".to_string(),
            summary: format!("Auto-generated CRUD workflows for {}", openapi.info.title),
            description: String::new(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: source_name,
            url: server_url,
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows,
        components: None,
        ..ArazzoSpec::default()
    };

    Ok(GenerateOutput {
        spec,
        warnings,
        resources: resource_names,
        auth_type: auth.as_ref().map(|a| a.scheme_type.clone()),
    })
}

// ─── Version Detection ───────────────────────────────────────────────────────

fn check_openapi_version(version: &str, warnings: &mut Vec<String>) -> Result<(), String> {
    if version.starts_with("2.") || version.starts_with("2,") {
        return Err(format!(
            "Swagger/OpenAPI 2.x is not supported (found \"{version}\"). \
             Please convert to OpenAPI 3.0 first."
        ));
    }
    if version.starts_with("3.1") {
        warnings.push(format!(
            "OpenAPI 3.1 detected (\"{version}\"); parsing with best-effort 3.0 compatibility. \
             Some 3.1-only features may not be recognized."
        ));
    }
    Ok(())
}

// ─── Server URL ──────────────────────────────────────────────────────────────

fn extract_server_url(openapi: &OpenAPI, warnings: &mut Vec<String>) -> Result<String, String> {
    let server = openapi.servers.first().ok_or_else(|| {
        "no servers defined in the OpenAPI spec; add a `servers` entry with an absolute URL"
            .to_string()
    })?;

    let mut url = server.url.clone();
    if url.is_empty() {
        return Err("server URL is empty; provide an absolute URL in the `servers` array".into());
    }

    if let Some(vars) = &server.variables {
        for (name, var) in vars {
            let placeholder = format!("{{{name}}}");
            if url.contains(&placeholder) {
                warnings.push(format!(
                    "server variable \"{name}\" substituted with default \"{}\"",
                    var.default
                ));
                url = url.replace(&placeholder, &var.default);
            }
        }
    }

    if url.starts_with('/') {
        return Err(format!(
            "server URL \"{url}\" is relative; use an absolute URL (e.g. https://api.example.com{url})"
        ));
    }

    let url = url.trim_end_matches('/').to_string();
    Ok(url)
}

// ─── Resource Grouping ───────────────────────────────────────────────────────

struct CrudOps {
    method: String,
    path: String,
    operation: openapiv3::Operation,
}

struct ResourceGroup {
    name: String,
    collection_path: String,
    item_path: Option<String>,
    id_param: Option<String>,
    create: Option<CrudOps>,
    list: Option<CrudOps>,
    read: Option<CrudOps>,
    update: Option<CrudOps>,
    delete: Option<CrudOps>,
}

impl ResourceGroup {
    fn step_count(&self) -> usize {
        usize::from(self.create.is_some())
            + usize::from(self.list.is_some())
            + usize::from(self.read.is_some())
            + usize::from(self.update.is_some())
            + usize::from(self.delete.is_some())
    }
}

fn count_path_params(path: &str) -> usize {
    path.split('/')
        .filter(|seg| seg.starts_with('{') && seg.ends_with('}'))
        .count()
}

fn trailing_param(path: &str) -> Option<String> {
    let last = path.rsplit('/').next()?;
    if last.starts_with('{') && last.ends_with('}') {
        Some(last[1..last.len() - 1].to_string())
    } else {
        None
    }
}

fn strip_trailing_param(path: &str) -> Option<String> {
    let idx = path.rfind('/')?;
    let prefix = &path[..idx];
    if prefix.is_empty() {
        Some("/".to_string())
    } else {
        Some(prefix.to_string())
    }
}

fn resource_name_from_path(path: &str) -> String {
    for seg in path.rsplit('/') {
        if !seg.is_empty() && !seg.starts_with('{') {
            return seg.to_string();
        }
    }
    "resource".to_string()
}

fn group_resources(openapi: &OpenAPI, warnings: &mut Vec<String>) -> Vec<ResourceGroup> {
    struct PathOp {
        path: String,
        method: String,
        operation: openapiv3::Operation,
    }

    let mut ops = Vec::new();
    for (path_str, path_item_ref) in &openapi.paths.paths {
        let path_item = match path_item_ref {
            ReferenceOr::Item(item) => item,
            ReferenceOr::Reference { .. } => continue,
        };

        let methods: Vec<(&str, Option<&openapiv3::Operation>)> = vec![
            ("GET", path_item.get.as_ref()),
            ("POST", path_item.post.as_ref()),
            ("PUT", path_item.put.as_ref()),
            ("PATCH", path_item.patch.as_ref()),
            ("DELETE", path_item.delete.as_ref()),
        ];

        for (method, maybe_op) in methods {
            if let Some(op) = maybe_op {
                ops.push(PathOp {
                    path: path_str.clone(),
                    method: method.to_string(),
                    operation: op.clone(),
                });
            }
        }
    }

    let mut collection_ops: BTreeMap<String, Vec<PathOp>> = BTreeMap::new();
    let mut item_ops: BTreeMap<String, Vec<PathOp>> = BTreeMap::new();

    for op in ops {
        let param_count = count_path_params(&op.path);

        if param_count >= 2 {
            warnings.push(format!(
                "skipping nested resource path \"{}\" (Phase 3)",
                op.path
            ));
            continue;
        }

        if trailing_param(&op.path).is_some() {
            let collection = strip_trailing_param(&op.path).unwrap_or_default();
            item_ops.entry(collection).or_default().push(op);
        } else {
            collection_ops.entry(op.path.clone()).or_default().push(op);
        }
    }

    let all_collection_paths: HashSet<String> = collection_ops.keys().cloned().collect();
    let all_item_prefixes: HashSet<String> = item_ops.keys().cloned().collect();
    let all_paths: HashSet<String> = all_collection_paths
        .union(&all_item_prefixes)
        .cloned()
        .collect();

    let mut groups = Vec::new();

    for collection_path in &all_paths {
        let name = resource_name_from_path(collection_path);
        let col_ops = collection_ops.remove(collection_path.as_str());
        let itm_ops = item_ops.remove(collection_path.as_str());

        let mut group = ResourceGroup {
            name: name.clone(),
            collection_path: collection_path.clone(),
            item_path: None,
            id_param: None,
            create: None,
            list: None,
            read: None,
            update: None,
            delete: None,
        };

        if let Some(col) = col_ops {
            for op in col {
                match op.method.as_str() {
                    "POST" => {
                        group.create = Some(CrudOps {
                            method: op.method,
                            path: op.path,
                            operation: op.operation,
                        })
                    }
                    "GET" => {
                        group.list = Some(CrudOps {
                            method: op.method,
                            path: op.path,
                            operation: op.operation,
                        })
                    }
                    _ => {}
                }
            }
        }

        if let Some(itm) = itm_ops {
            for op in itm {
                let param = trailing_param(&op.path);
                let full_item_path = op.path.clone();

                if group.item_path.is_none() {
                    group.item_path = Some(full_item_path.clone());
                    group.id_param = param;
                }

                match op.method.as_str() {
                    "GET" => {
                        group.read = Some(CrudOps {
                            method: op.method,
                            path: full_item_path,
                            operation: op.operation,
                        })
                    }
                    "PUT" | "PATCH" if group.update.is_none() || op.method == "PUT" => {
                        group.update = Some(CrudOps {
                            method: op.method,
                            path: full_item_path,
                            operation: op.operation,
                        });
                    }
                    "DELETE" => {
                        group.delete = Some(CrudOps {
                            method: op.method,
                            path: full_item_path,
                            operation: op.operation,
                        })
                    }
                    _ => {}
                }
            }
        }

        if group.step_count() >= 2 {
            groups.push(group);
        }
    }

    groups.sort_by(|a, b| a.collection_path.cmp(&b.collection_path));
    groups
}

// ─── Authentication Detection ────────────────────────────────────────────────

struct AuthRequirement {
    input_name: String,
    param_name: String,
    param_in: ParamLocation,
    param_value_expr: String,
    scheme_type: String,
}

fn detect_auth(openapi: &OpenAPI) -> Option<AuthRequirement> {
    let scheme_name = openapi.security.as_ref()?.first()?.keys().next()?.clone();

    let components = openapi.components.as_ref()?;
    let scheme_ref = components.security_schemes.get(&scheme_name)?;
    let scheme = match scheme_ref {
        ReferenceOr::Item(s) => s,
        ReferenceOr::Reference { .. } => return None,
    };

    match scheme {
        openapiv3::SecurityScheme::APIKey { location, name, .. } => {
            let param_in = match location {
                openapiv3::APIKeyLocation::Header => ParamLocation::Header,
                openapiv3::APIKeyLocation::Query => ParamLocation::Query,
                openapiv3::APIKeyLocation::Cookie => ParamLocation::Cookie,
            };
            Some(AuthRequirement {
                input_name: scheme_name.clone(),
                param_name: name.clone(),
                param_in,
                param_value_expr: format!("$inputs.{scheme_name}"),
                scheme_type: "apiKey".to_string(),
            })
        }
        openapiv3::SecurityScheme::HTTP {
            scheme: http_scheme,
            ..
        } => {
            let lower = http_scheme.to_lowercase();
            match lower.as_str() {
                "bearer" => Some(AuthRequirement {
                    input_name: "token".to_string(),
                    param_name: "Authorization".to_string(),
                    param_in: ParamLocation::Header,
                    param_value_expr: "Bearer {$inputs.token}".to_string(),
                    scheme_type: "http/bearer".to_string(),
                }),
                "basic" => Some(AuthRequirement {
                    input_name: "credentials".to_string(),
                    param_name: "Authorization".to_string(),
                    param_in: ParamLocation::Header,
                    param_value_expr: "Basic {$inputs.credentials}".to_string(),
                    scheme_type: "http/basic".to_string(),
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

// ─── Success Status Code ─────────────────────────────────────────────────────

fn extract_success_code(responses: &openapiv3::Responses, method: &str) -> u16 {
    let mut found_codes: Vec<u16> = Vec::new();

    for (status, _) in &responses.responses {
        if let StatusCode::Code(code) = status {
            if (200..300).contains(code) {
                found_codes.push(*code);
            }
        }
    }

    if found_codes.is_empty() {
        return match method {
            "POST" => 201,
            "DELETE" => 204,
            _ => 200,
        };
    }

    if method == "POST" && found_codes.contains(&201) {
        return 201;
    }
    if method == "DELETE" && found_codes.contains(&204) {
        return 204;
    }

    found_codes.sort_unstable();
    found_codes[0]
}

// ─── ID Field Heuristic ──────────────────────────────────────────────────────

fn find_id_field(group: &ResourceGroup, openapi: &OpenAPI) -> (String, String) {
    let path_param = group.id_param.clone().unwrap_or_else(|| "id".to_string());

    if let Some(ref create_op) = group.create {
        if let Some(field) = find_id_in_response(&create_op.operation, &openapi.components) {
            return (field, path_param);
        }
    }

    (path_param.clone(), path_param)
}

fn find_id_in_response(
    operation: &openapiv3::Operation,
    components: &Option<openapiv3::Components>,
) -> Option<String> {
    for (status, resp_ref) in &operation.responses.responses {
        let is_success = match status {
            StatusCode::Code(c) => (200..300).contains(c),
            StatusCode::Range(_) => false,
        };
        if !is_success {
            continue;
        }

        let resp = resolve_response_ref(resp_ref, components, &mut HashSet::new())?;
        let content = resp.content.get("application/json")?;
        let schema_ref = content.schema.as_ref()?;
        let mut visited = HashSet::new();
        let schema = resolve_schema_ref(schema_ref, components, &mut visited)?;

        if let openapiv3::SchemaKind::Type(openapiv3::Type::Object(obj)) = &schema.schema_kind {
            for name in obj.properties.keys() {
                if name == "id"
                    || name.ends_with("Id")
                    || name.ends_with("_id")
                    || name.ends_with("ID")
                {
                    return Some(name.clone());
                }
            }
        }
    }
    None
}

// ─── Source Name ─────────────────────────────────────────────────────────────

fn derive_source_name(title: &str, filename: &str) -> String {
    let from_title: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let mut from_title = from_title.trim_matches('-').to_string();
    while from_title.contains("--") {
        from_title = from_title.replace("--", "-");
    }

    if !from_title.is_empty() && from_title.len() <= 30 {
        return from_title;
    }

    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("api");
    let stem = stem
        .trim_end_matches(".openapi")
        .trim_end_matches(".swagger")
        .trim_end_matches(".oas");
    stem.to_string()
}

// ─── Workflow Assembly ───────────────────────────────────────────────────────

fn build_workflow(
    group: &ResourceGroup,
    source_name: &str,
    auth: &Option<AuthRequirement>,
    openapi: &OpenAPI,
) -> Workflow {
    let workflow_id = format!("crud-{}", group.name);
    let (id_body_field, id_param_name) = find_id_field(group, openapi);
    let has_create = group.create.is_some();

    let mut properties = BTreeMap::new();
    let mut required = Vec::new();

    if let Some(auth_req) = auth {
        properties.insert(
            auth_req.input_name.clone(),
            PropertyDef {
                type_: Some(JsonSchemaType::String),
                description: format!("Authentication value for {}", auth_req.param_name),
                ..PropertyDef::default()
            },
        );
        required.push(auth_req.input_name.clone());
    }

    if !has_create {
        if let Some(ref _item_path) = group.item_path {
            properties.insert(
                id_param_name.clone(),
                PropertyDef {
                    type_: Some(JsonSchemaType::String),
                    description: format!("ID of the {} resource", group.name),
                    ..PropertyDef::default()
                },
            );
            required.push(id_param_name.clone());
        }
    }

    let inputs = if properties.is_empty() {
        None
    } else {
        Some(SchemaObject {
            type_: Some(JsonSchemaType::Object),
            properties,
            required,
            ..SchemaObject::default()
        })
    };

    let mut wf_parameters = Vec::new();
    if let Some(auth_req) = auth {
        wf_parameters.push(Parameter {
            name: auth_req.param_name.clone(),
            in_: Some(auth_req.param_in),
            value: serde_yaml_ng::Value::String(auth_req.param_value_expr.clone()).into(),
            reference: String::new(),
            ..Parameter::default()
        });
    }

    let mut steps = Vec::new();

    if let Some(ref create) = group.create {
        steps.push(build_step(
            &format!("create-{}", group.name),
            &format!("Create a new {}", group.name),
            &create.method,
            &create.path,
            source_name,
            Some(&create.operation),
            &openapi.components,
            Some(&id_body_field),
        ));
    }

    if let Some(ref list) = group.list {
        steps.push(build_step(
            &format!("list-{}", group.name),
            &format!("List all {}", group.name),
            &list.method,
            &list.path,
            source_name,
            Some(&list.operation),
            &openapi.components,
            None,
        ));
    }

    if let Some(ref read) = group.read {
        let mut step = build_step(
            &format!("read-{}", group.name),
            &format!("Get a single {}", group.name),
            &read.method,
            &read.path,
            source_name,
            Some(&read.operation),
            &openapi.components,
            None,
        );
        if let Some(ref param_name) = group.id_param {
            let id_expr = if has_create {
                format!("$steps.create-{}.outputs.{id_body_field}", group.name)
            } else {
                format!("$inputs.{id_param_name}")
            };
            step.parameters.push(Parameter {
                name: param_name.clone(),
                in_: Some(ParamLocation::Path),
                value: serde_yaml_ng::Value::String(id_expr).into(),
                reference: String::new(),
                ..Parameter::default()
            });
        }
        steps.push(step);
    }

    if let Some(ref update) = group.update {
        let mut step = build_step(
            &format!("update-{}", group.name),
            &format!("Update a {}", group.name),
            &update.method,
            &update.path,
            source_name,
            Some(&update.operation),
            &openapi.components,
            None,
        );
        if let Some(ref param_name) = group.id_param {
            let id_expr = if has_create {
                format!("$steps.create-{}.outputs.{id_body_field}", group.name)
            } else {
                format!("$inputs.{id_param_name}")
            };
            step.parameters.push(Parameter {
                name: param_name.clone(),
                in_: Some(ParamLocation::Path),
                value: serde_yaml_ng::Value::String(id_expr).into(),
                reference: String::new(),
                ..Parameter::default()
            });
        }
        steps.push(step);
    }

    if let Some(ref delete) = group.delete {
        let mut step = build_step(
            &format!("delete-{}", group.name),
            &format!("Delete a {}", group.name),
            &delete.method,
            &delete.path,
            source_name,
            Some(&delete.operation),
            &openapi.components,
            None,
        );
        if let Some(ref param_name) = group.id_param {
            let id_expr = if has_create {
                format!("$steps.create-{}.outputs.{id_body_field}", group.name)
            } else {
                format!("$inputs.{id_param_name}")
            };
            step.parameters.push(Parameter {
                name: param_name.clone(),
                in_: Some(ParamLocation::Path),
                value: serde_yaml_ng::Value::String(id_expr).into(),
                reference: String::new(),
                ..Parameter::default()
            });
        }
        steps.push(step);
    }

    let mut outputs = BTreeMap::new();
    if has_create {
        outputs.insert(
            "created_id".to_string(),
            format!("$steps.create-{}.outputs.{id_body_field}", group.name).into(),
        );
    }

    Workflow {
        workflow_id,
        summary: format!("CRUD operations for {}", group.name),
        description: String::new(),
        inputs,
        steps,
        outputs,
        success_actions: Vec::new(),
        failure_actions: Vec::new(),
        parameters: wf_parameters,
        ..Workflow::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn build_step(
    step_id: &str,
    description: &str,
    method: &str,
    path: &str,
    source_name: &str,
    operation: Option<&openapiv3::Operation>,
    components: &Option<openapiv3::Components>,
    output_id_field: Option<&str>,
) -> Step {
    let operation_path = format!("{method} {{{source_name}}}.{path}");
    let status_code = operation
        .map(|op| extract_success_code(&op.responses, method))
        .unwrap_or_else(|| match method {
            "POST" => 201,
            "DELETE" => 204,
            _ => 200,
        });

    let request_body = operation.and_then(|op| {
        let rb_ref = op.request_body.as_ref()?;
        let rb = crate::refs::resolve_request_body_ref(rb_ref, components, &mut HashSet::new())?;
        let json_content = rb.content.get("application/json")?;
        let schema_ref = json_content.schema.as_ref()?;
        let example = generate_example(schema_ref, "body", components, 0);

        Some(RequestBody {
            content_type: "application/json".to_string(),
            payload: Some(json_to_yml(example).into()),
            reference: String::new(),
            ..RequestBody::default()
        })
    });

    let success_criteria = vec![SuccessCriterion {
        condition: format!("$statusCode == {status_code}"),
        context: String::new(),
        type_: None,
        ..SuccessCriterion::default()
    }];

    let on_failure = vec![OnAction {
        name: "fail-fast".to_string(),
        type_: Some(ActionType::End),
        ..OnAction::default()
    }];

    let mut outputs = BTreeMap::new();
    if let Some(id_field) = output_id_field {
        outputs.insert(
            id_field.to_string(),
            format!("$response.body.{id_field}").into(),
        );
    }

    Step {
        step_id: step_id.to_string(),
        description: description.to_string(),
        target: Some(StepTarget::OperationPath(operation_path)),
        parameters: Vec::new(),
        request_body,
        success_criteria,
        on_success: Vec::new(),
        on_failure,
        outputs,
        ..Step::default()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn parse_openapi(yaml: &str) -> OpenAPI {
        serde_yaml_ng::from_str(yaml).unwrap_or_else(|e| panic!("parse error: {e}"))
    }

    #[test]
    fn test_resource_grouping_basic() {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0"
servers:
  - url: https://api.example.com
paths:
  /items:
    get:
      operationId: listItems
      responses:
        "200":
          description: OK
    post:
      operationId: createItem
      responses:
        "201":
          description: Created
  /items/{itemId}:
    get:
      operationId: getItem
      responses:
        "200":
          description: OK
    delete:
      operationId: deleteItem
      responses:
        "204":
          description: Deleted
"#;
        let openapi = parse_openapi(yaml);
        let mut warnings = Vec::new();
        let groups = group_resources(&openapi, &mut warnings);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "items");
        assert!(groups[0].create.is_some());
        assert!(groups[0].list.is_some());
        assert!(groups[0].read.is_some());
        assert!(groups[0].delete.is_some());
        assert_eq!(groups[0].id_param, Some("itemId".to_string()));
    }

    #[test]
    fn test_nested_resources_skipped() {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0"
servers:
  - url: https://api.example.com
paths:
  /stores/{storeId}/items/{itemId}:
    get:
      operationId: getStoreItem
      responses:
        "200":
          description: OK
"#;
        let openapi = parse_openapi(yaml);
        let mut warnings = Vec::new();
        let groups = group_resources(&openapi, &mut warnings);

        assert!(groups.is_empty());
        assert!(warnings.iter().any(|w| w.contains("nested resource")));
    }

    #[test]
    fn test_extract_success_code_post_201() {
        let mut responses = openapiv3::Responses::default();
        responses.responses.insert(
            StatusCode::Code(201),
            ReferenceOr::Item(openapiv3::Response {
                description: "Created".to_string(),
                ..openapiv3::Response::default()
            }),
        );
        assert_eq!(extract_success_code(&responses, "POST"), 201);
    }

    #[test]
    fn test_extract_success_code_delete_204() {
        let mut responses = openapiv3::Responses::default();
        responses.responses.insert(
            StatusCode::Code(204),
            ReferenceOr::Item(openapiv3::Response {
                description: "Deleted".to_string(),
                ..openapiv3::Response::default()
            }),
        );
        assert_eq!(extract_success_code(&responses, "DELETE"), 204);
    }

    #[test]
    fn test_extract_success_code_fallback() {
        let responses = openapiv3::Responses::default();
        assert_eq!(extract_success_code(&responses, "GET"), 200);
        assert_eq!(extract_success_code(&responses, "POST"), 201);
        assert_eq!(extract_success_code(&responses, "DELETE"), 204);
    }

    #[test]
    fn test_auth_detection_api_key() {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0"
servers:
  - url: https://api.example.com
security:
  - ApiKeyAuth: []
paths: {}
components:
  securitySchemes:
    ApiKeyAuth:
      type: apiKey
      in: header
      name: X-API-Key
"#;
        let openapi = parse_openapi(yaml);
        let auth = detect_auth(&openapi);
        assert!(auth.is_some());
        let auth = auth.unwrap();
        assert_eq!(auth.input_name, "ApiKeyAuth");
        assert_eq!(auth.param_name, "X-API-Key");
        assert_eq!(auth.param_in, ParamLocation::Header);
        assert_eq!(auth.param_value_expr, "$inputs.ApiKeyAuth");
    }

    #[test]
    fn test_auth_detection_bearer() {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0"
servers:
  - url: https://api.example.com
security:
  - BearerAuth: []
paths: {}
components:
  securitySchemes:
    BearerAuth:
      type: http
      scheme: bearer
"#;
        let openapi = parse_openapi(yaml);
        let auth = detect_auth(&openapi);
        assert!(auth.is_some());
        let auth = auth.unwrap();
        assert_eq!(auth.input_name, "token");
        assert_eq!(auth.param_name, "Authorization");
        assert_eq!(auth.param_value_expr, "Bearer {$inputs.token}");
    }

    #[test]
    fn test_server_url_extraction() {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0"
servers:
  - url: https://api.example.com/v1/
paths: {}
"#;
        let openapi = parse_openapi(yaml);
        let mut warnings = Vec::new();
        let url = extract_server_url(&openapi, &mut warnings).unwrap();
        assert_eq!(url, "https://api.example.com/v1");
    }

    #[test]
    fn test_server_url_with_variables() {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0"
servers:
  - url: "https://{host}/v1"
    variables:
      host:
        default: api.example.com
paths: {}
"#;
        let openapi = parse_openapi(yaml);
        let mut warnings = Vec::new();
        let url = extract_server_url(&openapi, &mut warnings).unwrap();
        assert_eq!(url, "https://api.example.com/v1");
        assert!(warnings.iter().any(|w| w.contains("host")));
    }

    #[test]
    fn test_derive_source_name() {
        assert_eq!(derive_source_name("Petstore", "spec.yaml"), "petstore");
        assert_eq!(
            derive_source_name("My Cool API", "spec.yaml"),
            "my-cool-api"
        );
        assert_eq!(derive_source_name("", "petstore.openapi.yaml"), "petstore");
    }

    #[test]
    fn test_full_generation_petstore() {
        let yaml = include_str!("../../../testdata/petstore.openapi.yaml");
        let openapi: OpenAPI =
            serde_yaml_ng::from_str(yaml).unwrap_or_else(|e| panic!("parse error: {e}"));
        let result = generate_crud(&openapi, "petstore.openapi.yaml")
            .unwrap_or_else(|e| panic!("generate error: {e}"));

        assert_eq!(result.spec.arazzo, "1.0.0");
        assert!(!result.spec.workflows.is_empty());
        assert!(!result.resources.is_empty());
        assert!(result.auth_type.is_some());

        for wf in &result.spec.workflows {
            for step in &wf.steps {
                assert!(
                    !step.on_failure.is_empty(),
                    "step {} missing onFailure",
                    step.step_id
                );
            }
        }

        let yaml_out = serde_yaml_ng::to_string(&result.spec)
            .unwrap_or_else(|e| panic!("serialize error: {e}"));
        assert!(yaml_out.contains("arazzo:"));
        assert!(yaml_out.contains("crud-pets"));

        assert!(
            yaml_out.contains("{petstore}."),
            "operationPath must use {{sourceName}} prefix"
        );
        assert!(
            !yaml_out.contains("extensions:"),
            "generated specs should not emit empty extension maps"
        );
    }
}
