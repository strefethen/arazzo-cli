//! OpenAPI → Arazzo CRUD workflow generator.
//!
//! Given an OpenAPI 3.0 spec, produces a runnable Arazzo 1.1 document with
//! CRUD workflows, chained steps, authentication setup, and realistic request
//! bodies derived from schema examples.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

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
///
/// `document_url` is the value written into `sourceDescriptions[].url`: a
/// relative path — per the Arazzo Specification's Source Description Object,
/// a URI-reference per RFC 3986 §4.2 — that resolves to the OpenAPI document
/// `openapi` was parsed from, relative to the directory the generated Arazzo
/// document will itself live in. Callers compute it (see
/// [`relative_document_url`] for the CLI's file-writing case); this function
/// only writes it through, since it has no notion of where its own output
/// will land.
pub fn generate_crud(
    openapi: &OpenAPI,
    spec_filename: &str,
    document_url: &str,
) -> Result<GenerateOutput, String> {
    let mut warnings = Vec::new();

    check_openapi_version(&openapi.openapi, &mut warnings)?;
    // The server URL is no longer written into the generated document (that
    // baked the API's base URL into `sourceDescriptions[].url`, which the
    // Arazzo Specification reserves for a URL to the source description
    // itself). It is still extracted so the existing diagnostics — no
    // `servers` entry, an empty server URL, a relative server URL, and
    // server-variable substitution — keep firing at generation time.
    extract_server_url(openapi, &mut warnings)?;
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
        arazzo: "1.1.0".to_string(),
        info: Info {
            title: format!("{} Workflows", openapi.info.title),
            version: "1.0.0".to_string(),
            summary: format!("Auto-generated CRUD workflows for {}", openapi.info.title),
            description: String::new(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: source_name,
            url: document_url.to_string(),
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

// ─── Source Document URL ─────────────────────────────────────────────────────

/// Computes the `sourceDescriptions[].url` that points at the OpenAPI document
/// `spec_path` was read from, relative to `output_dir` — the directory the
/// generated Arazzo document will itself live in (an `--output` file's parent,
/// or the current working directory when writing to stdout).
///
/// `spec_path` and `output_dir` may each be relative or absolute; `cwd`
/// resolves any relative input to an absolute path before diffing. It is a
/// parameter rather than read from the process so this stays hermetic and
/// testable — callers pass `std::env::current_dir()`.
///
/// Each resolved path is canonicalized when it exists on disk, and only
/// falls back to a lexical (non-symlink-aware) normalization otherwise —
/// e.g. in tests that use paths which do not exist. A relative `..`-count
/// computed from un-resolved symlinked components (such as macOS's
/// `/tmp` → `/private/tmp`) can undercount how many directories up the *real*
/// common ancestor sits, which then resolves to the wrong file once the
/// runtime later walks that same relative path from the (possibly still
/// symlinked) document directory — canonicalizing first keeps the `..` count
/// correct regardless of which spelling either side was given in.
///
/// The Arazzo Specification's Source Description Object requires a relative
/// `url` to be a URI-reference (RFC 3986 §4.2); the runtime resolves it as a
/// raw filesystem path with no percent-decoding
/// (`runtime_core/builder.rs::load_document_source`), so this function never
/// percent-encodes its output — a spec filename that needs escaping to be a
/// strict URI-reference is a known, accepted deviation.
pub fn relative_document_url(
    spec_path: &str,
    output_dir: &Path,
    cwd: &Path,
) -> Result<String, String> {
    // Invariant that keeps a mixed canonicalize/lexical-fallback pairing
    // unreachable through shipped code: the CLI is this function's only
    // caller, and by the time it calls this, `spec_path` has already been
    // read successfully (so it canonicalizes) and `output_dir` must already
    // exist, because the later `fs::write` of the generated file has no
    // directory-creation step and fails before anything is persisted if it
    // does not. Both sides canonicalize, or neither does. A mix is only
    // reachable by calling this function directly, as the unit tests below
    // do with paths chosen not to exist on either side.
    let resolve = |path: &Path| -> PathBuf {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        std::fs::canonicalize(&absolute).unwrap_or_else(|_| normalize_lexically(&absolute))
    };

    let abs_spec = resolve(Path::new(spec_path));
    let abs_output_dir = resolve(output_dir);

    let spec_components: Vec<Component> = abs_spec.components().collect();
    let output_components: Vec<Component> = abs_output_dir.components().collect();

    let common = spec_components
        .iter()
        .zip(output_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    if common == 0 {
        return Err(format!(
            "cannot express OpenAPI document \"{spec_path}\" as a path relative to \"{}\"; \
             they share no common root",
            output_dir.display()
        ));
    }

    let mut rel = PathBuf::new();
    for _ in &output_components[common..] {
        rel.push("..");
    }
    for comp in &spec_components[common..] {
        rel.push(comp.as_os_str());
    }

    if rel.as_os_str().is_empty() {
        return Err(format!(
            "OpenAPI document \"{spec_path}\" resolves to the generated document's own \
             directory, not a file within it"
        ));
    }

    // The value is a URI-reference (RFC 3986 §4.2), not a platform filesystem
    // path — always join with `/` so generated files stay portable across
    // machines regardless of the host OS.
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Ok(guard_uri_scheme_ambiguity(&parts.join("/")))
}

/// Prefixes `./` when `url`'s first path segment contains a `:`.
///
/// A relative-path reference whose first segment contains a colon is not a
/// valid RFC 3986 §4.2 relative reference — per §4.2, it is indistinguishable
/// from an absolute URI's `scheme:`, so a spec named e.g. `api:v2.yaml` would
/// emit `url: api:v2.yaml`. The runtime's own classifier,
/// `is_relative_document_source` (`runtime_core/builder.rs`), tests
/// `Url::parse(&sd.url) == Err(RelativeUrlWithoutBase)` to decide "is this a
/// document to load"; `Url::parse("api:v2.yaml")` succeeds (scheme `api`,
/// opaque path `v2.yaml`), so that document would silently be treated as an
/// absolute base URL instead of loaded, and request URLs would resolve to
/// `api:v2.yaml/...` instead of the document's real `servers[0]`.
///
/// `./` sidesteps this without a runtime change: `Url::parse("./api:v2.yaml")`
/// fails with `RelativeUrlWithoutBase` (document semantics restored), and
/// `base_dir.join("./x")` resolves identically to `base_dir.join("x")` — `./`
/// is a no-op path component to any filesystem join.
pub fn guard_uri_scheme_ambiguity(url: &str) -> String {
    let first_segment = url.split('/').next().unwrap_or(url);
    if first_segment.contains(':') {
        format!("./{url}")
    } else {
        url.to_string()
    }
}

/// Resolves `.` and `..` components without touching the filesystem — neither
/// path need exist. `..` cancels a preceding normal component; it is kept
/// (and accumulates) when there is nothing to cancel, e.g. leading `../..`
/// segments in an already-relative absolute-joined path.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.last(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push(comp);
                }
            }
            other => out.push(other),
        }
    }
    out.into_iter().collect()
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
    use std::fs;

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
    fn test_server_url_extraction_missing_servers() {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0"
paths: {}
"#;
        let openapi = parse_openapi(yaml);
        let mut warnings = Vec::new();
        let err = extract_server_url(&openapi, &mut warnings).unwrap_err();
        assert!(
            err.contains("no servers defined"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_server_url_extraction_empty() {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0"
servers:
  - url: ""
paths: {}
"#;
        let openapi = parse_openapi(yaml);
        let mut warnings = Vec::new();
        let err = extract_server_url(&openapi, &mut warnings).unwrap_err();
        assert!(
            err.contains("server URL is empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_server_url_extraction_relative() {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0"
servers:
  - url: "/v1"
paths: {}
"#;
        let openapi = parse_openapi(yaml);
        let mut warnings = Vec::new();
        let err = extract_server_url(&openapi, &mut warnings).unwrap_err();
        assert!(err.contains("is relative"), "unexpected error: {err}");
    }

    // These use a path root guaranteed not to exist on the test host, which
    // exercises the lexical-normalization fallback (as opposed to the
    // canonicalizing fast path — see `test_relative_document_url_resolves_
    // through_symlinked_directories` below for that one).
    const FAKE_ROOT: &str = "/nonexistent-ac91284-root";

    #[test]
    fn test_relative_document_url_same_directory() {
        let url = relative_document_url(
            "openapi.yaml",
            &PathBuf::from(FAKE_ROOT).join("work"),
            &PathBuf::from(FAKE_ROOT).join("work"),
        )
        .unwrap();
        assert_eq!(url, "openapi.yaml");
    }

    #[test]
    fn test_relative_document_url_output_in_subdirectory() {
        // `--output out/generated.yaml` from cwd `<root>/work`, spec relative
        // in cwd.
        let url = relative_document_url(
            "openapi.yaml",
            &PathBuf::from(FAKE_ROOT).join("work/out"),
            &PathBuf::from(FAKE_ROOT).join("work"),
        )
        .unwrap();
        assert_eq!(url, "../openapi.yaml");
    }

    #[test]
    fn test_relative_document_url_output_in_unrelated_absolute_directory() {
        // `--output <root>/elsewhere/out/generated.yaml` from cwd
        // `<root>/work`, spec relative to a `specs/` subdirectory of cwd.
        let url = relative_document_url(
            "specs/petstore.yaml",
            &PathBuf::from(FAKE_ROOT).join("elsewhere/out"),
            &PathBuf::from(FAKE_ROOT).join("work"),
        )
        .unwrap();
        assert_eq!(url, "../../work/specs/petstore.yaml");
    }

    #[test]
    fn test_relative_document_url_absolute_spec_is_relativized() {
        let url = relative_document_url(
            &format!("{FAKE_ROOT}/specs/openapi.yaml"),
            &PathBuf::from(FAKE_ROOT).join("work"),
            &PathBuf::from(FAKE_ROOT).join("work"),
        )
        .unwrap();
        assert_eq!(url, "../specs/openapi.yaml");
        assert!(
            !Path::new(&url).is_absolute(),
            "emitted url must never be an absolute filesystem path: {url}"
        );
    }

    #[test]
    fn test_relative_document_url_dot_segments_normalized() {
        let url = relative_document_url(
            "./specs/../openapi.yaml",
            &PathBuf::from(FAKE_ROOT).join("work"),
            &PathBuf::from(FAKE_ROOT).join("work"),
        )
        .unwrap();
        assert_eq!(url, "openapi.yaml");
    }

    #[test]
    fn test_guard_uri_scheme_ambiguity_prefixes_dot_slash_when_first_segment_has_colon() {
        // "api:v2.yaml" alone parses as a URI with scheme "api" — not a
        // relative-path reference (RFC 3986 §4.2) — so is_relative_document_
        // source (runtime_core/builder.rs) would treat it as an absolute base
        // url instead of a document to load.
        assert_eq!(guard_uri_scheme_ambiguity("api:v2.yaml"), "./api:v2.yaml");
        // A colon-bearing first segment after `..` segments is still the
        // first segment of the *string* — RFC 3986 §3.1 scheme ambiguity
        // reads left to right, so this is ambiguous too.
        assert_eq!(
            guard_uri_scheme_ambiguity("api:v2.yaml/pets"),
            "./api:v2.yaml/pets"
        );
    }

    #[test]
    fn test_guard_uri_scheme_ambiguity_leaves_unambiguous_urls_untouched() {
        assert_eq!(guard_uri_scheme_ambiguity("openapi.yaml"), "openapi.yaml");
        assert_eq!(
            guard_uri_scheme_ambiguity("../openapi.yaml"),
            "../openapi.yaml"
        );
        // A colon past the first segment is not ambiguous with a scheme.
        assert_eq!(
            guard_uri_scheme_ambiguity("specs/api:v2.yaml"),
            "specs/api:v2.yaml"
        );
    }

    #[test]
    fn test_relative_document_url_guards_colon_bearing_spec_filename() {
        // A spec named with a colon in its first path segment (e.g. an
        // OpenAPI-style versioned file name) must come back `./`-prefixed.
        // This only checks the string transformation; the proof that it
        // keeps the document loadable by the real runtime classifier
        // (`is_relative_document_source`, which parses the url with the
        // `url` crate) is the CLI's end-to-end
        // `generate_dry_run_round_trip_with_colon_bearing_spec_filename`,
        // which exercises the actual compiled runtime.
        let url = relative_document_url(
            "api:v2.yaml",
            &PathBuf::from(FAKE_ROOT).join("work"),
            &PathBuf::from(FAKE_ROOT).join("work"),
        )
        .unwrap();
        assert_eq!(url, "./api:v2.yaml");
    }

    #[test]
    fn test_relative_document_url_resolves_through_symlinked_directories() {
        // Regression: a purely lexical `..`-count is wrong when a path
        // component is a symlink (e.g. macOS's `/tmp` -> `/private/tmp`) —
        // `..` always walks the *real* directory graph. Reproduce that shape
        // with a private symlink so the test does not depend on the host's
        // `/tmp` layout, then prove the emitted url actually resolves back
        // to the spec file the way `EngineBuilder`'s document loader would
        // join it (`base_dir.join(&sd.url)`).
        let base = std::env::temp_dir().join(format!(
            "ac91284-relative-document-url-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let real_root = base.join("real-root");
        let spec_dir = real_root.join("specs");
        fs::create_dir_all(&spec_dir).unwrap_or_else(|e| panic!("creating spec dir: {e}"));
        let spec_path = spec_dir.join("openapi.yaml");
        fs::write(&spec_path, b"openapi: 3.0.3\n").unwrap_or_else(|e| panic!("writing spec: {e}"));

        let link_root = base.join("link-root");
        symlink_dir(&real_root, &link_root);

        let output_dir = link_root.join("out");
        fs::create_dir_all(&output_dir).unwrap_or_else(|e| panic!("creating output dir: {e}"));

        let cwd = std::env::current_dir().unwrap_or_else(|e| panic!("reading cwd: {e}"));
        let url = relative_document_url(&spec_path.to_string_lossy(), &output_dir, &cwd)
            .unwrap_or_else(|e| panic!("relative_document_url: {e}"));

        // Resolve the emitted url exactly as the runtime does — join it onto
        // the (possibly still-symlinked) generated document's directory —
        // and confirm it lands on the real spec file.
        let resolved = output_dir.join(&url);
        let resolved_real = fs::canonicalize(&resolved)
            .unwrap_or_else(|e| panic!("canonicalizing {resolved:?}: {e} (url was {url:?})"));
        let spec_real =
            fs::canonicalize(&spec_path).unwrap_or_else(|e| panic!("canonicalizing spec: {e}"));
        assert_eq!(
            resolved_real, spec_real,
            "relative url {url:?} from {output_dir:?} did not resolve back to {spec_path:?}"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    fn symlink_dir(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link)
            .unwrap_or_else(|e| panic!("symlinking {link:?} -> {target:?}: {e}"));
    }

    #[cfg(not(unix))]
    fn symlink_dir(target: &Path, link: &Path) {
        std::os::windows::fs::symlink_dir(target, link)
            .unwrap_or_else(|e| panic!("symlinking {link:?} -> {target:?}: {e}"));
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
        let result = generate_crud(&openapi, "petstore.openapi.yaml", "petstore.openapi.yaml")
            .unwrap_or_else(|e| panic!("generate error: {e}"));

        assert_eq!(result.spec.arazzo, "1.1.0");
        assert!(!result.spec.workflows.is_empty());
        assert!(!result.resources.is_empty());
        assert!(result.auth_type.is_some());

        // The source description points at the OpenAPI document, not the
        // API's base URL (petstore.openapi.yaml declares
        // `servers[0].url: https://petstore.example.com/v1`, which must not
        // appear here).
        assert_eq!(result.spec.source_descriptions.len(), 1);
        assert_eq!(
            result.spec.source_descriptions[0].url,
            "petstore.openapi.yaml"
        );

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
        assert!(yaml_out.contains("url: petstore.openapi.yaml"));
        assert!(
            !yaml_out.contains("https://petstore.example.com"),
            "the API base URL must not be baked into the generated document: {yaml_out}"
        );

        assert!(
            yaml_out.contains("{petstore}."),
            "operationPath must use {{sourceName}} prefix"
        );
        assert!(
            !yaml_out.contains("extensions:"),
            "generated specs should not emit empty extension maps"
        );
    }

    #[test]
    fn test_full_generation_server_url_with_path_prefix_and_variables_unaffected() {
        // servers[0].url carries both a path prefix and a variable default;
        // extract_server_url's substitution/diagnostics still run even
        // though its result is no longer written into the document — this
        // is the "generation-time diagnostics are preserved" guarantee.
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Variable API
  version: "1.0"
servers:
  - url: "https://{host}/v2"
    variables:
      host:
        default: api.example.com
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
"#;
        let openapi = parse_openapi(yaml);
        let result = generate_crud(&openapi, "variable-api.yaml", "variable-api.yaml")
            .unwrap_or_else(|e| panic!("generate error: {e}"));

        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("host") && w.contains("api.example.com")),
            "expected server-variable substitution warning, got: {:?}",
            result.warnings
        );
        assert_eq!(result.spec.source_descriptions[0].url, "variable-api.yaml");
        assert!(!result.spec.source_descriptions[0].url.contains("http"));
    }
}
