//! OpenAPI `$ref` resolution utilities.

use std::collections::HashSet;

use openapiv3::ReferenceOr;

pub fn resolve_schema_ref<'a>(
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

pub fn resolve_request_body_ref<'a>(
    rb_ref: &'a ReferenceOr<openapiv3::RequestBody>,
    components: &'a Option<openapiv3::Components>,
    visited: &mut HashSet<String>,
) -> Option<&'a openapiv3::RequestBody> {
    match rb_ref {
        ReferenceOr::Item(rb) => Some(rb),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/requestBodies/")?;
            if !visited.insert(name.to_string()) {
                return None; // cycle
            }
            let comps = components.as_ref()?;
            let next_ref = comps.request_bodies.get(name)?;
            resolve_request_body_ref(next_ref, components, visited)
        }
    }
}

pub fn resolve_response_ref<'a>(
    resp_ref: &'a ReferenceOr<openapiv3::Response>,
    components: &'a Option<openapiv3::Components>,
    visited: &mut HashSet<String>,
) -> Option<&'a openapiv3::Response> {
    match resp_ref {
        ReferenceOr::Item(resp) => Some(resp),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/responses/")?;
            if !visited.insert(name.to_string()) {
                return None; // cycle
            }
            let comps = components.as_ref()?;
            let next_ref = comps.responses.get(name)?;
            resolve_response_ref(next_ref, components, visited)
        }
    }
}

/// Convert `ReferenceOr<Box<Schema>>` to `ReferenceOr<Schema>`.
pub fn ref_box_to_ref(r: &ReferenceOr<Box<openapiv3::Schema>>) -> ReferenceOr<openapiv3::Schema> {
    match r {
        ReferenceOr::Item(schema) => ReferenceOr::Item(*schema.clone()),
        ReferenceOr::Reference { reference } => ReferenceOr::Reference {
            reference: reference.clone(),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

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
    fn test_request_body_ref_resolution_with_cycle() {
        let mut request_bodies = IndexMap::new();
        request_bodies.insert(
            "A".to_string(),
            ReferenceOr::Reference {
                reference: "#/components/requestBodies/A".to_string(),
            },
        );
        let components = Some(openapiv3::Components {
            request_bodies,
            ..openapiv3::Components::default()
        });

        let ref_ = ReferenceOr::Reference::<openapiv3::RequestBody> {
            reference: "#/components/requestBodies/A".to_string(),
        };
        let result = resolve_request_body_ref(&ref_, &components, &mut HashSet::new());
        assert!(result.is_none());
    }

    #[test]
    fn test_response_ref_resolution_with_cycle() {
        let mut responses = IndexMap::new();
        responses.insert(
            "A".to_string(),
            ReferenceOr::Reference {
                reference: "#/components/responses/A".to_string(),
            },
        );
        let components = Some(openapiv3::Components {
            responses,
            ..openapiv3::Components::default()
        });

        let ref_ = ReferenceOr::Reference::<openapiv3::Response> {
            reference: "#/components/responses/A".to_string(),
        };
        let result = resolve_response_ref(&ref_, &components, &mut HashSet::new());
        assert!(result.is_none());
    }
}
