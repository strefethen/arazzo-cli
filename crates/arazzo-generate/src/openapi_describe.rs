//! OpenAPI spec introspection — endpoints, schemas, and auth schemes.

use std::collections::HashSet;

use openapiv3::{OpenAPI, ReferenceOr};
use serde_json::{json, Value};

use crate::refs::resolve_schema_ref;

/// Describe an OpenAPI spec: endpoints, schemas, and auth schemes.
pub fn describe(openapi: &OpenAPI) -> Value {
    let server_url = openapi
        .servers
        .first()
        .map(|s| s.url.as_str())
        .unwrap_or("");

    json!({
        "title": openapi.info.title,
        "version": openapi.info.version,
        "server_url": server_url,
        "endpoints": collect_endpoints(openapi),
        "schemas": collect_schemas(openapi),
        "auth_schemes": collect_auth_schemes(openapi),
    })
}

fn collect_endpoints(openapi: &OpenAPI) -> Vec<Value> {
    let mut endpoints = Vec::new();

    for (path, item_ref) in &openapi.paths.paths {
        let item = match item_ref {
            ReferenceOr::Item(item) => item,
            ReferenceOr::Reference { .. } => continue,
        };

        let methods: &[(&str, Option<&openapiv3::Operation>)] = &[
            ("GET", item.get.as_ref()),
            ("POST", item.post.as_ref()),
            ("PUT", item.put.as_ref()),
            ("PATCH", item.patch.as_ref()),
            ("DELETE", item.delete.as_ref()),
            ("HEAD", item.head.as_ref()),
            ("OPTIONS", item.options.as_ref()),
        ];

        for &(method, maybe_op) in methods {
            if let Some(op) = maybe_op {
                let tags: Vec<&str> = op.tags.iter().map(String::as_str).collect();
                endpoints.push(json!({
                    "path": path,
                    "method": method,
                    "operation_id": op.operation_id,
                    "summary": op.summary,
                    "tags": tags,
                    "has_request_body": op.request_body.is_some(),
                }));
            }
        }
    }

    endpoints
}

fn collect_schemas(openapi: &OpenAPI) -> Vec<Value> {
    let components = match &openapi.components {
        Some(c) => c,
        None => return Vec::new(),
    };

    let mut schemas = Vec::new();

    for (name, schema_ref) in &components.schemas {
        let mut visited = HashSet::new();
        let schema = match resolve_schema_ref(schema_ref, &openapi.components, &mut visited) {
            Some(s) => s,
            None => {
                schemas.push(json!({
                    "name": name,
                    "type": "unresolved",
                    "properties": [],
                    "required": [],
                }));
                continue;
            }
        };

        match &schema.schema_kind {
            openapiv3::SchemaKind::Type(openapiv3::Type::Object(obj)) => {
                let props: Vec<Value> = obj
                    .properties
                    .iter()
                    .map(|(prop_name, prop_ref)| {
                        let (typ, fmt) = extract_type_info(prop_ref, &openapi.components);
                        json!({
                            "name": prop_name,
                            "type": typ,
                            "format": fmt,
                        })
                    })
                    .collect();
                schemas.push(json!({
                    "name": name,
                    "type": "object",
                    "properties": props,
                    "required": obj.required,
                }));
            }
            openapiv3::SchemaKind::AllOf { .. }
            | openapiv3::SchemaKind::OneOf { .. }
            | openapiv3::SchemaKind::AnyOf { .. } => {
                schemas.push(json!({
                    "name": name,
                    "type": "composite",
                    "properties": [],
                    "required": [],
                }));
            }
            _ => {
                let type_name = match &schema.schema_kind {
                    openapiv3::SchemaKind::Type(t) => match t {
                        openapiv3::Type::String(_) => "string",
                        openapiv3::Type::Integer(_) => "integer",
                        openapiv3::Type::Number(_) => "number",
                        openapiv3::Type::Boolean(_) => "boolean",
                        openapiv3::Type::Array(_) => "array",
                        openapiv3::Type::Object(_) => unreachable!(),
                    },
                    _ => "unknown",
                };
                schemas.push(json!({
                    "name": name,
                    "type": type_name,
                    "properties": [],
                    "required": [],
                }));
            }
        }
    }

    schemas
}

fn extract_type_info(
    prop_ref: &ReferenceOr<Box<openapiv3::Schema>>,
    components: &Option<openapiv3::Components>,
) -> (String, Value) {
    let schema_ref = crate::refs::ref_box_to_ref(prop_ref);
    let mut visited = HashSet::new();
    let schema = match resolve_schema_ref(&schema_ref, components, &mut visited) {
        Some(s) => s,
        None => return ("$ref".to_string(), Value::Null),
    };

    match &schema.schema_kind {
        openapiv3::SchemaKind::Type(t) => match t {
            openapiv3::Type::String(s) => {
                let fmt = match &s.format {
                    openapiv3::VariantOrUnknownOrEmpty::Item(f) => {
                        json!(format!("{f:?}").to_lowercase())
                    }
                    openapiv3::VariantOrUnknownOrEmpty::Unknown(f) => json!(f),
                    openapiv3::VariantOrUnknownOrEmpty::Empty => Value::Null,
                };
                ("string".to_string(), fmt)
            }
            openapiv3::Type::Integer(i) => {
                let fmt = match &i.format {
                    openapiv3::VariantOrUnknownOrEmpty::Item(f) => {
                        json!(format!("{f:?}").to_lowercase())
                    }
                    openapiv3::VariantOrUnknownOrEmpty::Unknown(f) => json!(f),
                    openapiv3::VariantOrUnknownOrEmpty::Empty => Value::Null,
                };
                ("integer".to_string(), fmt)
            }
            openapiv3::Type::Number(_) => ("number".to_string(), Value::Null),
            openapiv3::Type::Boolean(_) => ("boolean".to_string(), Value::Null),
            openapiv3::Type::Array(_) => ("array".to_string(), Value::Null),
            openapiv3::Type::Object(_) => ("object".to_string(), Value::Null),
        },
        _ => ("unknown".to_string(), Value::Null),
    }
}

fn collect_auth_schemes(openapi: &OpenAPI) -> Vec<Value> {
    let components = match &openapi.components {
        Some(c) => c,
        None => return Vec::new(),
    };

    let mut schemes = Vec::new();

    for (name, scheme_ref) in &components.security_schemes {
        let scheme = match scheme_ref {
            ReferenceOr::Item(s) => s,
            ReferenceOr::Reference { .. } => continue,
        };

        let info = match scheme {
            openapiv3::SecurityScheme::APIKey {
                location,
                name: param_name,
                ..
            } => {
                let in_ = match location {
                    openapiv3::APIKeyLocation::Header => "header",
                    openapiv3::APIKeyLocation::Query => "query",
                    openapiv3::APIKeyLocation::Cookie => "cookie",
                };
                json!({
                    "name": name,
                    "type": "apiKey",
                    "in": in_,
                    "param_name": param_name,
                    "scheme": null,
                })
            }
            openapiv3::SecurityScheme::HTTP {
                scheme: http_scheme,
                ..
            } => {
                json!({
                    "name": name,
                    "type": "http",
                    "in": null,
                    "param_name": null,
                    "scheme": http_scheme,
                })
            }
            openapiv3::SecurityScheme::OAuth2 { .. } => {
                json!({
                    "name": name,
                    "type": "oauth2",
                    "in": null,
                    "param_name": null,
                    "scheme": null,
                })
            }
            openapiv3::SecurityScheme::OpenIDConnect {
                open_id_connect_url,
                ..
            } => {
                json!({
                    "name": name,
                    "type": "openIdConnect",
                    "in": null,
                    "param_name": open_id_connect_url,
                    "scheme": null,
                })
            }
        };

        schemes.push(info);
    }

    schemes
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> OpenAPI {
        serde_yaml_ng::from_str(yaml).unwrap_or_else(|e| panic!("parse error: {e}"))
    }

    #[test]
    fn test_describe_endpoints() {
        let openapi = parse(
            r#"
openapi: "3.0.3"
info:
  title: Test API
  version: "1.0"
servers:
  - url: https://api.example.com
paths:
  /pets:
    get:
      operationId: listPets
      summary: List all pets
      tags: [pets]
      responses:
        "200":
          description: OK
    post:
      operationId: createPet
      summary: Create a pet
      tags: [pets]
      requestBody:
        content:
          application/json:
            schema:
              type: object
      responses:
        "201":
          description: Created
"#,
        );

        let result = describe(&openapi);
        let endpoints = result["endpoints"].as_array().unwrap();
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0]["method"], "GET");
        assert_eq!(endpoints[0]["operation_id"], "listPets");
        assert_eq!(endpoints[1]["has_request_body"], true);
    }

    #[test]
    fn test_describe_schemas() {
        let openapi = parse(
            r#"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0"
servers:
  - url: https://api.example.com
paths: {}
components:
  schemas:
    Pet:
      type: object
      required: [name]
      properties:
        id:
          type: integer
          format: int64
        name:
          type: string
"#,
        );

        let result = describe(&openapi);
        let schemas = result["schemas"].as_array().unwrap();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0]["name"], "Pet");
        assert_eq!(schemas[0]["type"], "object");

        let props = schemas[0]["properties"].as_array().unwrap();
        assert_eq!(props.len(), 2);

        let required = schemas[0]["required"].as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "name");
    }

    #[test]
    fn test_describe_auth_schemes() {
        let openapi = parse(
            r#"
openapi: "3.0.3"
info:
  title: Test
  version: "1.0"
servers:
  - url: https://api.example.com
paths: {}
components:
  securitySchemes:
    ApiKeyAuth:
      type: apiKey
      in: header
      name: X-API-Key
    BearerAuth:
      type: http
      scheme: bearer
"#,
        );

        let result = describe(&openapi);
        let auth = result["auth_schemes"].as_array().unwrap();
        assert_eq!(auth.len(), 2);
    }
}
