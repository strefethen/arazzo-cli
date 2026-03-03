//! OpenAPI → Arazzo CRUD workflow generator (Phase 1).
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
use indexmap::IndexMap;
use openapiv3::{OpenAPI, ReferenceOr, StatusCode};
use serde_json::Value;

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

    // ── Version check ────────────────────────────────────────────────────
    check_openapi_version(&openapi.openapi, &mut warnings)?;

    // ── Server URL ───────────────────────────────────────────────────────
    let server_url = extract_server_url(openapi, &mut warnings)?;

    // ── Resource grouping ────────────────────────────────────────────────
    let groups = group_resources(openapi, &mut warnings);
    if groups.is_empty() {
        return Err("no CRUD resource groups found in the OpenAPI spec".to_string());
    }

    // ── Authentication ───────────────────────────────────────────────────
    let auth = detect_auth(openapi);

    // ── Source description name ──────────────────────────────────────────
    let source_name = derive_source_name(&openapi.info.title, spec_filename);

    // ── Build Arazzo spec ────────────────────────────────────────────────
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
        },
        source_descriptions: vec![SourceDescription {
            name: source_name,
            url: server_url,
            type_: SourceType::OpenApi,
        }],
        workflows,
        components: None,
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

    // Substitute server variables with their defaults.
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

    // Strip trailing slash for consistency.
    let url = url.trim_end_matches('/').to_string();

    Ok(url)
}

// ─── $ref Resolution ─────────────────────────────────────────────────────────

fn resolve_schema_ref<'a>(
    schema_ref: &'a ReferenceOr<openapiv3::Schema>,
    components: &'a Option<openapiv3::Components>,
    visited: &mut HashSet<String>,
) -> Option<&'a openapiv3::Schema> {
    match schema_ref {
        ReferenceOr::Item(schema) => Some(schema),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/schemas/")?;
            if !visited.insert(name.to_string()) {
                return None; // cycle
            }
            let comps = components.as_ref()?;
            let next_ref = comps.schemas.get(name)?;
            resolve_schema_ref(next_ref, components, visited)
        }
    }
}

fn resolve_request_body_ref<'a>(
    rb_ref: &'a ReferenceOr<openapiv3::RequestBody>,
    components: &'a Option<openapiv3::Components>,
) -> Option<&'a openapiv3::RequestBody> {
    match rb_ref {
        ReferenceOr::Item(rb) => Some(rb),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/requestBodies/")?;
            let comps = components.as_ref()?;
            let next_ref = comps.request_bodies.get(name)?;
            resolve_request_body_ref(next_ref, components)
        }
    }
}

fn resolve_response_ref<'a>(
    resp_ref: &'a ReferenceOr<openapiv3::Response>,
    components: &'a Option<openapiv3::Components>,
) -> Option<&'a openapiv3::Response> {
    match resp_ref {
        ReferenceOr::Item(resp) => Some(resp),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/responses/")?;
            let comps = components.as_ref()?;
            let next_ref = comps.responses.get(name)?;
            resolve_response_ref(next_ref, components)
        }
    }
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

/// Counts `{param}` segments in a path.
fn count_path_params(path: &str) -> usize {
    path.split('/')
        .filter(|seg| seg.starts_with('{') && seg.ends_with('}'))
        .count()
}

/// Extracts the trailing `{param}` name from a path, if present.
fn trailing_param(path: &str) -> Option<String> {
    let last = path.rsplit('/').next()?;
    if last.starts_with('{') && last.ends_with('}') {
        Some(last[1..last.len() - 1].to_string())
    } else {
        None
    }
}

/// Strips the trailing `/{param}` to get the collection path.
fn strip_trailing_param(path: &str) -> Option<String> {
    let idx = path.rfind('/')?;
    let prefix = &path[..idx];
    if prefix.is_empty() {
        Some("/".to_string())
    } else {
        Some(prefix.to_string())
    }
}

/// Derives the resource name from the last non-param segment.
fn resource_name_from_path(path: &str) -> String {
    for seg in path.rsplit('/') {
        if !seg.is_empty() && !seg.starts_with('{') {
            return seg.to_string();
        }
    }
    "resource".to_string()
}

fn group_resources(openapi: &OpenAPI, warnings: &mut Vec<String>) -> Vec<ResourceGroup> {
    // Collect all (path, method, operation) tuples.
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

    // Separate item paths (trailing {param}) from collection paths.
    let mut collection_ops: BTreeMap<String, Vec<PathOp>> = BTreeMap::new();
    let mut item_ops: BTreeMap<String, Vec<PathOp>> = BTreeMap::new();

    for op in ops {
        let param_count = count_path_params(&op.path);

        // Skip nested resources (2+ path params).
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

    // Build resource groups by matching collection + item paths.
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

        // Process collection operations.
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
                    _ => {} // Ignore other methods on collection
                }
            }
        }

        // Process item operations.
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
                    "PUT" | "PATCH" => {
                        // Prefer PUT over PATCH if both exist.
                        if group.update.is_none() || op.method == "PUT" {
                            group.update = Some(CrudOps {
                                method: op.method,
                                path: full_item_path,
                                operation: op.operation,
                            });
                        }
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

        // Require at least two CRUD operations to form a meaningful workflow.
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
    // Get the first security requirement name from global security.
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
        _ => None, // OAuth2 etc. deferred to Phase 2
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

    // Prefer method-appropriate codes.
    if method == "POST" && found_codes.contains(&201) {
        return 201;
    }
    if method == "DELETE" && found_codes.contains(&204) {
        return 204;
    }

    found_codes.sort_unstable();
    found_codes[0]
}

// ─── Example Value Generation ────────────────────────────────────────────────

fn generate_example(
    schema_ref: &ReferenceOr<openapiv3::Schema>,
    field_name: &str,
    components: &Option<openapiv3::Components>,
    depth: usize,
) -> Value {
    if depth > 5 {
        return Value::Null;
    }

    let mut visited = HashSet::new();
    let schema = match resolve_schema_ref(schema_ref, components, &mut visited) {
        Some(s) => s,
        None => return Value::Null,
    };

    generate_example_from_schema(schema, field_name, components, depth)
}

fn generate_example_from_schema(
    schema: &openapiv3::Schema,
    field_name: &str,
    components: &Option<openapiv3::Components>,
    depth: usize,
) -> Value {
    if depth > 5 {
        return Value::Null;
    }

    // Check for explicit example.
    if let Some(example) = &schema.schema_data.example {
        return example.clone();
    }

    // Check for default.
    if let Some(default) = &schema.schema_data.default {
        return default.clone();
    }

    match &schema.schema_kind {
        openapiv3::SchemaKind::Type(type_info) => {
            generate_from_type(type_info, field_name, components, depth)
        }
        openapiv3::SchemaKind::Any(any) => generate_from_any(any, field_name, components, depth),
        _ => Value::Null,
    }
}

fn generate_from_type(
    type_info: &openapiv3::Type,
    field_name: &str,
    components: &Option<openapiv3::Components>,
    depth: usize,
) -> Value {
    match type_info {
        openapiv3::Type::String(s) => generate_string_example(field_name, &s.format),
        openapiv3::Type::Integer(_) => Value::Number(1.into()),
        openapiv3::Type::Number(_) => serde_json::json!(1.0),
        openapiv3::Type::Boolean(_) => Value::Bool(true),
        openapiv3::Type::Array(arr) => {
            if let Some(items) = &arr.items {
                let item_ref = match items {
                    ReferenceOr::Item(schema) => ReferenceOr::Item(*schema.clone()),
                    ReferenceOr::Reference { reference } => ReferenceOr::Reference {
                        reference: reference.clone(),
                    },
                };
                let item_val = generate_example(&item_ref, "item", components, depth + 1);
                Value::Array(vec![item_val])
            } else {
                Value::Array(vec![])
            }
        }
        openapiv3::Type::Object(obj) => {
            generate_object_example(&obj.properties, &obj.required, components, depth)
        }
    }
}

fn generate_from_any(
    any: &openapiv3::AnySchema,
    field_name: &str,
    components: &Option<openapiv3::Components>,
    depth: usize,
) -> Value {
    // If it has properties, treat as object.
    if !any.properties.is_empty() {
        return generate_object_example(&any.properties, &any.required, components, depth);
    }

    // Fall back to type hint.
    if let Some(ref ty) = any.typ {
        match ty.as_str() {
            "string" => {
                return generate_string_example(
                    field_name,
                    &openapiv3::VariantOrUnknownOrEmpty::Empty,
                )
            }
            "integer" => return Value::Number(1.into()),
            "number" => return serde_json::json!(1.0),
            "boolean" => return Value::Bool(true),
            _ => {}
        }
    }

    Value::Null
}

fn generate_object_example(
    properties: &IndexMap<String, ReferenceOr<Box<openapiv3::Schema>>>,
    required: &[String],
    components: &Option<openapiv3::Components>,
    depth: usize,
) -> Value {
    let mut obj = serde_json::Map::new();
    let mut count = 0;
    let max_optional = 5;

    // Required first.
    for name in required {
        if let Some(prop_ref) = properties.get(name) {
            let prop_ref = ref_box_to_ref(prop_ref);
            obj.insert(
                name.clone(),
                generate_example(&prop_ref, name, components, depth + 1),
            );
        }
    }

    // Then optional (up to limit).
    for (name, prop_ref) in properties {
        if required.contains(name) {
            continue;
        }
        if count >= max_optional {
            break;
        }
        let prop_ref = ref_box_to_ref(prop_ref);
        obj.insert(
            name.clone(),
            generate_example(&prop_ref, name, components, depth + 1),
        );
        count += 1;
    }

    Value::Object(obj)
}

/// Convert `ReferenceOr<Box<Schema>>` to `ReferenceOr<Schema>`.
fn ref_box_to_ref(r: &ReferenceOr<Box<openapiv3::Schema>>) -> ReferenceOr<openapiv3::Schema> {
    match r {
        ReferenceOr::Item(schema) => ReferenceOr::Item(*schema.clone()),
        ReferenceOr::Reference { reference } => ReferenceOr::Reference {
            reference: reference.clone(),
        },
    }
}

fn generate_string_example(
    field_name: &str,
    format: &openapiv3::VariantOrUnknownOrEmpty<openapiv3::StringFormat>,
) -> Value {
    match format {
        openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::StringFormat::DateTime) => {
            Value::String("2024-01-01T00:00:00Z".to_string())
        }
        openapiv3::VariantOrUnknownOrEmpty::Item(openapiv3::StringFormat::Date) => {
            Value::String("2024-01-01".to_string())
        }
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) if s == "email" => {
            Value::String("user@example.com".to_string())
        }
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) if s == "uuid" => {
            Value::String("550e8400-e29b-41d4-a716-446655440000".to_string())
        }
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) if s == "uri" || s == "url" => {
            Value::String("https://example.com".to_string())
        }
        _ => Value::String(format!("example-{field_name}")),
    }
}

// ─── ID Field Heuristic ──────────────────────────────────────────────────────

/// Returns (response_body_field, path_param_name).
/// `response_body_field` is used for `$response.body.{field}` extraction.
/// `path_param_name` is used for downstream `{param}` path substitution.
fn find_id_field(group: &ResourceGroup, openapi: &OpenAPI) -> (String, String) {
    let path_param = group.id_param.clone().unwrap_or_else(|| "id".to_string());

    // Check if the create response schema has the path param as a field.
    if let Some(ref create_op) = group.create {
        if let Some(field) = find_id_in_response(&create_op.operation, &openapi.components) {
            // Response has an id-like field — use it for extraction,
            // and the path param name for downstream path substitution.
            return (field, path_param);
        }
    }

    // Fall back: assume the path param name matches the response body field.
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

        let resp = resolve_response_ref(resp_ref, components)?;
        let content = resp.content.get("application/json")?;
        let schema_ref = content.schema.as_ref()?;
        let mut visited = HashSet::new();
        let schema = resolve_schema_ref(schema_ref, components, &mut visited)?;

        // Look for id-like properties.
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
    // Try to derive from title: lowercase, replace spaces with hyphens, keep alphanumeric.
    let from_title: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse consecutive dashes.
    let mut from_title = from_title.trim_matches('-').to_string();
    while from_title.contains("--") {
        from_title = from_title.replace("--", "-");
    }

    if !from_title.is_empty() && from_title.len() <= 30 {
        return from_title;
    }

    // Fall back to filename stem.
    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("api");
    // Remove common suffixes.
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

    // ── Inputs schema ────────────────────────────────────────────────────
    let mut properties = BTreeMap::new();
    let mut required = Vec::new();

    // Auth input.
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

    // If no create step, the user needs to provide the resource ID.
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
        })
    };

    // ── Workflow-level parameters (auth) ─────────────────────────────────
    let mut wf_parameters = Vec::new();
    if let Some(auth_req) = auth {
        wf_parameters.push(Parameter {
            name: auth_req.param_name.clone(),
            in_: Some(auth_req.param_in),
            value: serde_yml::Value::String(auth_req.param_value_expr.clone()),
            reference: String::new(),
        });
    }

    // ── Steps ────────────────────────────────────────────────────────────
    let mut steps = Vec::new();

    // CREATE
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

    // LIST
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

    // READ
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
        // Add path parameter for the ID.
        if let Some(ref param_name) = group.id_param {
            let id_expr = if has_create {
                format!("$steps.create-{}.outputs.{id_body_field}", group.name)
            } else {
                format!("$inputs.{id_param_name}")
            };
            step.parameters.push(Parameter {
                name: param_name.clone(),
                in_: Some(ParamLocation::Path),
                value: serde_yml::Value::String(id_expr),
                reference: String::new(),
            });
        }
        steps.push(step);
    }

    // UPDATE
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
                value: serde_yml::Value::String(id_expr),
                reference: String::new(),
            });
        }
        steps.push(step);
    }

    // DELETE
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
                value: serde_yml::Value::String(id_expr),
                reference: String::new(),
            });
        }
        steps.push(step);
    }

    // ── Outputs ──────────────────────────────────────────────────────────
    let mut outputs = BTreeMap::new();
    if has_create {
        outputs.insert(
            "created_id".to_string(),
            format!("$steps.create-{}.outputs.{id_body_field}", group.name),
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

    // Build request body if the operation has one.
    let request_body = operation.and_then(|op| {
        let rb_ref = op.request_body.as_ref()?;
        let rb = resolve_request_body_ref(rb_ref, components)?;
        let json_content = rb.content.get("application/json")?;
        let schema_ref = json_content.schema.as_ref()?;
        let example = generate_example(schema_ref, "body", components, 0);

        Some(RequestBody {
            content_type: "application/json".to_string(),
            payload: Some(json_to_yml(example)),
            reference: String::new(),
        })
    });

    // Success criteria: check status code.
    let success_criteria = vec![SuccessCriterion {
        condition: format!("$statusCode == {status_code}"),
        context: String::new(),
        type_: None,
    }];

    // onFailure: end immediately.
    let on_failure = vec![OnAction {
        name: "fail-fast".to_string(),
        type_: ActionType::End,
        ..OnAction::default()
    }];

    // Step outputs: extract the ID field from the response body.
    let mut outputs = BTreeMap::new();
    if let Some(id_field) = output_id_field {
        outputs.insert(id_field.to_string(), format!("$response.body.{id_field}"));
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
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Convert serde_json::Value to serde_yml::Value.
fn json_to_yml(v: Value) -> serde_yml::Value {
    match v {
        Value::Null => serde_yml::Value::Null,
        Value::Bool(b) => serde_yml::Value::Bool(b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_yml::Value::Number(serde_yml::Number::from(i))
            } else if let Some(u) = n.as_u64() {
                serde_yml::Value::Number(serde_yml::Number::from(u))
            } else if let Some(f) = n.as_f64() {
                serde_yml::Value::Number(serde_yml::Number::from(f))
            } else {
                serde_yml::Value::Null
            }
        }
        Value::String(s) => serde_yml::Value::String(s),
        Value::Array(arr) => serde_yml::Value::Sequence(arr.into_iter().map(json_to_yml).collect()),
        Value::Object(map) => {
            let mut m = serde_yml::Mapping::new();
            for (k, v) in map {
                m.insert(serde_yml::Value::String(k), json_to_yml(v));
            }
            serde_yml::Value::Mapping(m)
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn parse_openapi(yaml: &str) -> OpenAPI {
        serde_yml::from_str(yaml).unwrap_or_else(|e| panic!("parse error: {e}"))
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
    fn test_example_generation_string() {
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::String(
                openapiv3::StringType::default(),
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "myField", &None, 0);
        assert_eq!(result, Value::String("example-myField".to_string()));
    }

    #[test]
    fn test_example_generation_integer() {
        let schema = openapiv3::Schema {
            schema_data: openapiv3::SchemaData::default(),
            schema_kind: openapiv3::SchemaKind::Type(openapiv3::Type::Integer(
                openapiv3::IntegerType::default(),
            )),
        };
        let result = generate_example(&ReferenceOr::Item(schema), "count", &None, 0);
        assert_eq!(result, serde_json::json!(1));
    }

    #[test]
    fn test_ref_resolution_with_cycle() {
        let mut schemas = IndexMap::new();
        schemas.insert(
            "A".to_string(),
            ReferenceOr::Reference {
                reference: "#/components/schemas/A".to_string(),
            },
        );
        let components = Some(openapiv3::Components {
            schemas,
            ..openapiv3::Components::default()
        });

        let ref_ = ReferenceOr::Reference::<openapiv3::Schema> {
            reference: "#/components/schemas/A".to_string(),
        };
        let mut visited = HashSet::new();
        let result = resolve_schema_ref(&ref_, &components, &mut visited);
        assert!(result.is_none());
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
            serde_yml::from_str(yaml).unwrap_or_else(|e| panic!("parse error: {e}"));
        let result = generate_crud(&openapi, "petstore.openapi.yaml")
            .unwrap_or_else(|e| panic!("generate error: {e}"));

        assert_eq!(result.spec.arazzo, "1.0.0");
        assert!(!result.spec.workflows.is_empty());
        assert!(!result.resources.is_empty());
        assert!(result.auth_type.is_some());

        // Verify all steps have onFailure
        for wf in &result.spec.workflows {
            for step in &wf.steps {
                assert!(
                    !step.on_failure.is_empty(),
                    "step {} missing onFailure",
                    step.step_id
                );
            }
        }

        // Verify the generated spec serializes to valid YAML.
        let yaml_out =
            serde_yml::to_string(&result.spec).unwrap_or_else(|e| panic!("serialize error: {e}"));
        assert!(yaml_out.contains("arazzo:"));
        assert!(yaml_out.contains("crud-pets"));

        // Verify operationPath uses {sourceName} format
        assert!(
            yaml_out.contains("{petstore}."),
            "operationPath must use {{sourceName}} prefix"
        );
    }
}
