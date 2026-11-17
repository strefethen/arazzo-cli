use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BytesDescriptor {
    Content(ContentDescriptor),
    Redacted(RedactedDescriptor),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentDescriptor {
    kind: BytesKind,
    pub size: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedactedDescriptor {
    kind: BytesKind,
    redacted: Redacted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum BytesKind {
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "bool", into = "bool")]
struct Redacted;

impl TryFrom<bool> for Redacted {
    type Error = &'static str;

    fn try_from(value: bool) -> Result<Self, Self::Error> {
        if value {
            Ok(Self)
        } else {
            Err("a redacted bytes descriptor must have redacted: true")
        }
    }
}

impl From<Redacted> for bool {
    fn from(_: Redacted) -> Self {
        true
    }
}

impl BytesDescriptor {
    pub fn of(bytes: &[u8], media_type: Option<String>, filename: Option<String>) -> Self {
        Self::Content(ContentDescriptor {
            kind: BytesKind::Bytes,
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            sha256: sha256_hex(bytes),
            media_type,
            filename,
        })
    }

    pub fn redacted() -> Self {
        Self::Redacted(RedactedDescriptor {
            kind: BytesKind::Bytes,
            redacted: Redacted,
        })
    }

    pub fn summary(&self) -> String {
        match self {
            Self::Content(content) => match &content.media_type {
                Some(media_type) => format!("<{} bytes, {media_type}>", content.size),
                None => format!("<{} bytes>", content.size),
            },
            Self::Redacted(_) => "<bytes redacted>".to_owned(),
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn content_descriptor_carries_size_and_lowercase_sha256() {
        let descriptor = BytesDescriptor::of(b"abc", Some("image/png".into()), None);
        assert_eq!(
            serde_json::to_value(&descriptor).unwrap_or_else(|err| panic!("{err}")),
            json!({
                "kind": "bytes",
                "size": 3,
                "sha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                "mediaType": "image/png"
            })
        );
        assert_eq!(descriptor.summary(), "<3 bytes, image/png>");
    }

    #[test]
    fn empty_bytes_hash_and_optional_fields_are_omitted() {
        let descriptor = BytesDescriptor::of(b"", None, Some("a.bin".into()));
        assert_eq!(
            serde_json::to_value(&descriptor).unwrap_or_else(|err| panic!("{err}")),
            json!({
                "kind": "bytes",
                "size": 0,
                "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                "filename": "a.bin"
            })
        );
        assert_eq!(descriptor.summary(), "<0 bytes>");
    }

    #[test]
    fn redacted_marker_is_fixed_and_has_no_hash() {
        let marker = BytesDescriptor::redacted();
        let value = serde_json::to_value(&marker).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(value, json!({"kind": "bytes", "redacted": true}));
        assert_eq!(marker, BytesDescriptor::redacted());
        assert_eq!(marker.summary(), "<bytes redacted>");
    }

    #[test]
    fn deserialization_accepts_only_the_two_published_shapes() {
        let content = BytesDescriptor::of(b"x", None, None);
        let round_trip: BytesDescriptor = serde_json::from_value(
            serde_json::to_value(&content).unwrap_or_else(|err| panic!("{err}")),
        )
        .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(round_trip, content);
        let marker: BytesDescriptor =
            serde_json::from_value(json!({"kind": "bytes", "redacted": true}))
                .unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(marker, BytesDescriptor::redacted());
        for rejected in [
            json!({"kind": "bytes", "redacted": false}),
            json!({"kind": "blob", "size": 1, "sha256": "00"}),
            json!({"kind": "bytes", "size": 1}),
            json!({"kind": "bytes", "size": 1, "sha256": "00", "content": "eA=="}),
            json!({"kind": "bytes", "redacted": true, "sha256": "00"}),
        ] {
            assert!(
                serde_json::from_value::<BytesDescriptor>(rejected.clone()).is_err(),
                "{rejected} must not deserialize as a bytes descriptor"
            );
        }
    }
}
