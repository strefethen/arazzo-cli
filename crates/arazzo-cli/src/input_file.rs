use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arazzo_runtime::{is_schema_declared_secret, is_sensitive_key, Blob, BytesDescriptor};
use arazzo_spec::{JsonSchemaType, PropertyDef, SchemaObject};
use base64::engine::general_purpose::{STANDARD, URL_SAFE};
use base64::Engine as _;
use serde_json::Value;

const TYPE_SUFFIX: &str = ";type=";
const FILENAME_SUFFIX: &str = ";filename=";

pub struct InputFile {
    name: String,
    path: PathBuf,
    media_type: Option<String>,
    filename: Option<String>,
    bytes: Arc<[u8]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Undeclared {
    Reject,
    Skip,
}

#[derive(Debug, Clone, Default)]
pub struct FileInputs {
    pub blobs: BTreeMap<String, Blob>,
    pub values: BTreeMap<String, Value>,
}

pub fn read_all(
    flags: &[String],
    max_bytes: usize,
    json_inputs: &BTreeMap<String, Value>,
) -> Result<Vec<InputFile>, String> {
    let mut files: Vec<InputFile> = Vec::new();
    for flag in flags {
        let parsed = parse_flag(flag)?;
        if files.iter().any(|file| file.name == parsed.name) {
            return Err(format!(
                "--input-file names input \"{}\" more than once",
                parsed.name
            ));
        }
        if json_inputs.contains_key(&parsed.name) {
            return Err(format!(
                "input \"{}\" is given by --input-file and also by --input or --input-json; give it once",
                parsed.name
            ));
        }
        let bytes = read_capped(&parsed.name, &parsed.path, max_bytes)?;
        files.push(InputFile {
            bytes: bytes.into(),
            ..parsed
        });
    }
    Ok(files)
}

pub fn convert(
    files: &[InputFile],
    workflow_id: &str,
    schema: Option<&SchemaObject>,
    undeclared: Undeclared,
) -> Result<FileInputs, String> {
    let mut inputs = FileInputs::default();
    for file in files {
        let Some(property) = schema.and_then(|schema| schema.properties.get(&file.name)) else {
            if undeclared == Undeclared::Skip {
                continue;
            }
            return Err(format!(
                "--input-file names input \"{}\", which workflow \"{workflow_id}\" does not declare in its inputs",
                file.name
            ));
        };
        match file.row(property)? {
            Row::Bytes { media_type } => {
                inputs
                    .blobs
                    .insert(file.name.clone(), file.blob(property, media_type));
            }
            Row::Text(text) => {
                inputs.values.insert(file.name.clone(), Value::String(text));
            }
        }
    }
    Ok(inputs)
}

pub fn descriptors(blobs: &BTreeMap<String, Blob>) -> BTreeMap<String, BytesDescriptor> {
    blobs
        .iter()
        .map(|(name, blob)| (name.clone(), blob.descriptor()))
        .collect()
}

enum Row {
    Bytes { media_type: Option<String> },
    Text(String),
}

impl InputFile {
    fn refused(&self, reason: impl std::fmt::Display) -> String {
        format!("--input-file input \"{}\": {reason}", self.name)
    }

    fn row(&self, property: &PropertyDef) -> Result<Row, String> {
        match property.type_ {
            None => self.bytes_row(property),
            Some(JsonSchemaType::String) => self.string_row(property).map(Row::Text),
            Some(other) => Err(self.refused(format_args!(
                "its schema declares type \"{other}\"; a file loads only for a schema with no type (bytes) or type \"string\""
            ))),
        }
    }

    fn bytes_row(&self, property: &PropertyDef) -> Result<Row, String> {
        if let Some(keyword) = property
            .reference_or_applicator_keyword()
            .or_else(|| property.enumeration_keyword())
        {
            return Err(self.refused(format_args!(
                "its schema declares {keyword}, which this runtime does not resolve for a file input"
            )));
        }
        if property.content_encoding().is_some() {
            return Err(self.refused(
                "its schema declares contentEncoding without type \"string\"; an encoded file input needs type \"string\"",
            ));
        }
        let schema_media_type = match property.content_media_type() {
            None => None,
            Some(Ok(media_type)) => Some(media_type.to_owned()),
            Some(Err(_)) => {
                return Err(self.refused("its schema's contentMediaType is not a string"));
            }
        };
        Ok(Row::Bytes {
            media_type: self.media_type.clone().or(schema_media_type),
        })
    }

    fn string_row(&self, property: &PropertyDef) -> Result<String, String> {
        if let Some(keyword) = property.reference_or_applicator_keyword() {
            return Err(self.refused(format_args!(
                "its schema declares {keyword}, which this runtime does not resolve for a file input"
            )));
        }
        if self.media_type.is_some() || self.filename.is_some() {
            return Err(self.refused(
                "the ;type= and ;filename= suffixes apply only to a bytes input, and this input's schema declares type \"string\"",
            ));
        }
        match property.content_encoding() {
            None => String::from_utf8(self.bytes.to_vec()).map_err(|error| {
                self.refused(format_args!(
                    "{} is not valid UTF-8 (at byte {}), and the input's schema declares type \"string\" with no contentEncoding",
                    self.path.display(),
                    error.utf8_error().valid_up_to()
                ))
            }),
            Some(Ok("base64")) => Ok(STANDARD.encode(&self.bytes)),
            Some(Ok("base64url")) => Ok(URL_SAFE.encode(&self.bytes)),
            Some(Ok(other)) => Err(self.refused(format_args!(
                "its schema declares contentEncoding \"{other}\"; a file input supports base64 and base64url"
            ))),
            Some(Err(_)) => Err(self.refused("its schema's contentEncoding is not a string")),
        }
    }

    fn blob(&self, property: &PropertyDef, media_type: Option<String>) -> Blob {
        let mut blob = Blob::new(Arc::clone(&self.bytes));
        let filename = self.filename.clone().or_else(|| {
            self.path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });
        if let Some(filename) = filename {
            blob = blob.with_filename(filename);
        }
        if let Some(media_type) = media_type {
            blob = blob.with_media_type(media_type);
        }
        if is_schema_declared_secret(property) || is_sensitive_key(&self.name) {
            blob = blob.sensitive();
        }
        blob
    }
}

fn parse_flag(flag: &str) -> Result<InputFile, String> {
    let malformed = || {
        format!("invalid --input-file {flag:?}: expected <input-name>=<path>[;type=<media-type>][;filename=<name>]")
    };
    let (name, rest) = flag
        .split_once('=')
        .filter(|(name, _)| !name.is_empty())
        .ok_or_else(malformed)?;
    let (path, mut suffixes) = match next_suffix(rest) {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    if path.is_empty() {
        return Err(malformed());
    }
    let mut media_type = None;
    let mut filename = None;
    while !suffixes.is_empty() {
        let (slot, label, after) = if let Some(after) = suffixes.strip_prefix(TYPE_SUFFIX) {
            (&mut media_type, "type", after)
        } else if let Some(after) = suffixes.strip_prefix(FILENAME_SUFFIX) {
            (&mut filename, "filename", after)
        } else {
            return Err(malformed());
        };
        let end = next_suffix(after).unwrap_or(after.len());
        let value = &after[..end];
        if value.is_empty() {
            return Err(format!(
                "invalid --input-file {flag:?}: ;{label}= needs a value"
            ));
        }
        if slot.replace(value.to_owned()).is_some() {
            return Err(format!(
                "invalid --input-file {flag:?}: ;{label}= is given more than once"
            ));
        }
        suffixes = &after[end..];
    }
    Ok(InputFile {
        name: name.to_owned(),
        path: PathBuf::from(path),
        media_type,
        filename,
        bytes: Arc::from(Vec::new()),
    })
}

fn next_suffix(text: &str) -> Option<usize> {
    [TYPE_SUFFIX, FILENAME_SUFFIX]
        .into_iter()
        .filter_map(|suffix| text.find(suffix))
        .min()
}

fn read_capped(name: &str, path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let failed = |action: &str, error: std::io::Error| {
        format!(
            "--input-file input \"{name}\": {action} {}: {error}",
            path.display()
        )
    };
    let file = File::open(path).map_err(|error| failed("opening", error))?;
    if file
        .metadata()
        .map_err(|error| failed("reading", error))?
        .is_dir()
    {
        return Err(format!(
            "--input-file input \"{name}\": {} is a directory, not a file",
            path.display()
        ));
    }
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

    fn parsed(flag: &str) -> (String, String, Option<String>, Option<String>) {
        let file = parse_flag(flag).unwrap_or_else(|error| panic!("{flag}: {error}"));
        (
            file.name,
            file.path.display().to_string(),
            file.media_type,
            file.filename,
        )
    }

    #[test]
    fn the_path_runs_to_the_first_recognized_suffix() {
        let some = |text: &str| Some(text.to_owned());
        for (flag, path, media_type, filename) in [
            ("p=cat.jpg", "cat.jpg", None, None),
            ("p=a;b.jpg", "a;b.jpg", None, None),
            ("p=a;Type=x", "a;Type=x", None, None),
            ("p=a=b.jpg", "a=b.jpg", None, None),
            ("p=$HOME/x", "$HOME/x", None, None),
            (
                "p=cat.jpg;type=image/png",
                "cat.jpg",
                some("image/png"),
                None,
            ),
            ("p=cat.jpg;filename=up.jpg", "cat.jpg", None, some("up.jpg")),
            (
                "p=a;b;type=text/plain; charset=utf-8;filename=x;y.txt",
                "a;b",
                some("text/plain; charset=utf-8"),
                some("x;y.txt"),
            ),
            (
                "p=cat.jpg;filename=up.jpg;type=image/jpeg",
                "cat.jpg",
                some("image/jpeg"),
                some("up.jpg"),
            ),
        ] {
            assert_eq!(
                parsed(flag),
                ("p".to_owned(), path.to_owned(), media_type, filename),
                "{flag}"
            );
        }
    }

    #[test]
    fn a_malformed_flag_is_refused() {
        for (flag, expected) in [
            ("photo", "expected <input-name>=<path>"),
            ("=cat.jpg", "expected <input-name>=<path>"),
            ("photo=", "expected <input-name>=<path>"),
            ("photo=;type=image/png", "expected <input-name>=<path>"),
            ("photo=cat.jpg;type=", ";type= needs a value"),
            (
                "photo=cat.jpg;type=a;filename=b;type=c",
                ";type= is given more than once",
            ),
        ] {
            let error = parse_flag(flag)
                .err()
                .unwrap_or_else(|| panic!("{flag} parsed"));
            assert!(error.contains(expected), "{flag}: {error}");
        }
    }
}
