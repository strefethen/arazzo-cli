use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use arazzo_runtime::{is_schema_declared_secret, is_sensitive_key, Blob, BytesDescriptor};
use arazzo_spec::{PropertyDef, SchemaObject};
use serde_json::Value;

const KEYWORDS_THAT_RULE_OUT_BYTES: [&str; 8] = [
    "$ref", "enum", "const", "allOf", "anyOf", "oneOf", "not", "if",
];

pub struct InputFiles<'a> {
    pub flags: &'a [String],
    pub max_bytes: usize,
    pub workflow_id: &'a str,
    pub schema: Option<&'a SchemaObject>,
    pub json_inputs: &'a BTreeMap<String, Value>,
}

impl InputFiles<'_> {
    pub fn load(&self) -> Result<BTreeMap<String, Blob>, String> {
        let mut blobs = BTreeMap::new();
        for flag in self.flags {
            let (name, path) = flag
                .split_once('=')
                .filter(|(name, path)| !name.is_empty() && !path.is_empty())
                .ok_or_else(|| {
                    format!("invalid --input-file {flag:?}: expected <input-name>=<path>")
                })?;
            if blobs.contains_key(name) {
                return Err(format!(
                    "--input-file names input \"{name}\" more than once"
                ));
            }
            if self.json_inputs.contains_key(name) {
                return Err(format!(
                    "input \"{name}\" is given by --input-file and also by --input or --input-json; give it once"
                ));
            }
            let property = self
                .schema
                .and_then(|schema| schema.properties.get(name))
                .ok_or_else(|| {
                    format!(
                        "--input-file names input \"{name}\", which workflow \"{}\" does not declare in its inputs",
                        self.workflow_id
                    )
                })?;
            if let Some(declared) = declares_a_value_kind(property) {
                return Err(format!(
                    "--input-file input \"{name}\": its schema declares {declared}, which is not supported by this build; a file is loaded as bytes only for an input whose schema has no type, $ref, enum, const, allOf, anyOf, oneOf, not or if"
                ));
            }
            let mut blob = Blob::new(read_capped(name, Path::new(path), self.max_bytes)?);
            if let Some(filename) = Path::new(path).file_name() {
                blob = blob.with_filename(filename.to_string_lossy());
            }
            if is_schema_declared_secret(property) || is_sensitive_key(name) {
                blob = blob.sensitive();
            }
            blobs.insert(name.to_owned(), blob);
        }
        Ok(blobs)
    }
}

pub fn descriptors(blobs: &BTreeMap<String, Blob>) -> BTreeMap<String, BytesDescriptor> {
    blobs
        .iter()
        .map(|(name, blob)| (name.clone(), blob.descriptor()))
        .collect()
}

fn declares_a_value_kind(property: &PropertyDef) -> Option<String> {
    if let Some(type_) = property.type_ {
        return Some(format!("type \"{type_}\""));
    }
    KEYWORDS_THAT_RULE_OUT_BYTES
        .into_iter()
        .find(|keyword| property.extensions.contains_key(*keyword))
        .map(str::to_owned)
}

fn read_capped(name: &str, path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let failed = |action: &str, error: std::io::Error| {
        format!(
            "--input-file input \"{name}\": {action} {}: {error}",
            path.display()
        )
    };
    let file = File::open(path).map_err(|error| failed("opening", error))?;
    let limit = u64::try_from(max_bytes).unwrap_or(u64::MAX);
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| failed("reading", error))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "--input-file input \"{name}\": {} is larger than --max-input-file-size ({max_bytes} bytes)",
            path.display()
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn property(yaml: &str) -> PropertyDef {
        serde_yaml_ng::from_str(yaml).unwrap_or_else(|error| panic!("property fixture: {error}"))
    }

    #[test]
    fn only_a_schema_with_no_type_and_no_value_keyword_is_bytes() {
        for bytes in [
            "{}",
            "description: a photo",
            "format: binary",
            "contentMediaType: image/jpeg",
            "writeOnly: true",
            "default: x",
        ] {
            assert_eq!(declares_a_value_kind(&property(bytes)), None, "{bytes}");
        }
        for (other, declared) in [
            ("type: string", "type \"string\""),
            ("type: object", "type \"object\""),
            ("{type: string, contentEncoding: base64}", "type \"string\""),
            ("$ref: '#/components/schemas/Photo'", "$ref"),
            ("enum: [a, b]", "enum"),
            ("const: a", "const"),
            ("allOf: [{}]", "allOf"),
            ("anyOf: [{}]", "anyOf"),
            ("oneOf: [{}]", "oneOf"),
            ("not: {}", "not"),
            ("if: {}", "if"),
        ] {
            assert_eq!(
                declares_a_value_kind(&property(other)).as_deref(),
                Some(declared),
                "{other}"
            );
        }
    }
}
