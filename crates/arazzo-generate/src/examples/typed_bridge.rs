use openapiv3::{OpenAPI, ReferenceOr, Schema, SchemaKind, Type};
use serde_yaml_ng::{Mapping, Value};

pub(crate) fn root(openapi: &OpenAPI) -> Result<Value, String> {
    let mut root = serde_yaml_ng::to_value(openapi).map_err(|e| e.to_string())?;
    if let Some(components) = &openapi.components {
        let mut schemas = Mapping::new();
        for (name, schema_ref) in &components.schemas {
            schemas.insert(Value::String(name.clone()), schema(schema_ref)?);
        }
        root["components"]["schemas"] = Value::Mapping(schemas);
    }
    Ok(root)
}

pub(crate) fn schema(schema_ref: &ReferenceOr<Schema>) -> Result<Value, String> {
    match schema_ref {
        ReferenceOr::Reference { reference } => Ok(reference_tree(reference)),
        ReferenceOr::Item(value) => schema_tree(value),
    }
}

fn reference_tree(reference: &str) -> Value {
    let Some(name) = reference.strip_prefix("#/components/schemas/") else {
        return Value::Null;
    };
    let name = name.replace('~', "~0").replace('/', "~1");
    let mut value = Mapping::new();
    value.insert(
        Value::String("$ref".into()),
        Value::String(format!("#/components/schemas/{name}")),
    );
    Value::Mapping(value)
}

fn schema_tree(schema: &Schema) -> Result<Value, String> {
    if schema
        .schema_data
        .example
        .as_ref()
        .is_some_and(serde_json::Value::is_null)
        || (schema.schema_data.example.is_none()
            && schema
                .schema_data
                .default
                .as_ref()
                .is_some_and(serde_json::Value::is_null))
    {
        return Ok(Value::Null);
    }
    let mut value = serde_yaml_ng::to_value(schema).map_err(|e| e.to_string())?;
    let (properties, required, items) = match &schema.schema_kind {
        SchemaKind::Type(Type::Object(object)) => {
            (Some(&object.properties), Some(&object.required), None)
        }
        SchemaKind::Type(Type::Array(array)) => (None, None, array.items.as_ref()),
        SchemaKind::Any(any) => {
            let mut normalized = Mapping::new();
            for key in ["example", "default"] {
                if let Some(literal) = value.get(key) {
                    normalized.insert(Value::String(key.into()), literal.clone());
                }
            }
            if !any.properties.is_empty() {
                normalized.insert(Value::String("type".into()), Value::String("object".into()));
            } else if let Some(typ @ ("string" | "integer" | "number" | "boolean")) =
                any.typ.as_deref()
            {
                normalized.insert(Value::String("type".into()), Value::String(typ.into()));
            }
            value = Value::Mapping(normalized);
            if any.properties.is_empty() {
                return Ok(value);
            }
            (Some(&any.properties), Some(&any.required), None)
        }
        SchemaKind::Type(_) => return Ok(value),
        _ => {
            let mut normalized = Mapping::new();
            for key in ["example", "default"] {
                if let Some(literal) = value.get(key) {
                    normalized.insert(Value::String(key.into()), literal.clone());
                }
            }
            return Ok(Value::Mapping(normalized));
        }
    };
    if let Some(properties) = properties {
        let mut normalized = Mapping::new();
        for (name, property) in properties {
            let property = match property {
                ReferenceOr::Reference { reference } => reference_tree(reference),
                ReferenceOr::Item(property) => schema_tree(property)?,
            };
            normalized.insert(Value::String(name.clone()), property);
        }
        value["properties"] = Value::Mapping(normalized);
    }
    if let Some(required) = required {
        value["required"] = serde_yaml_ng::to_value(required).map_err(|e| e.to_string())?;
    }
    if let Some(items) = items {
        value["items"] = match items {
            ReferenceOr::Reference { reference } => reference_tree(reference),
            ReferenceOr::Item(item) => schema_tree(item)?,
        };
    }
    Ok(value)
}
