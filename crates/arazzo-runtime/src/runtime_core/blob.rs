use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

use serde_json::Value;

use super::BytesDescriptor;

pub(crate) const BYTES_SCOPE_MARKER: &str = "bytes (not viewable until descriptor support)";

#[derive(Clone)]
pub struct Blob {
    source: Source,
    filename: Option<String>,
    media_type: Option<String>,
    sensitive: bool,
    content: Arc<OnceLock<BytesDescriptor>>,
}

#[derive(Clone)]
enum Source {
    Memory(Arc<[u8]>),
}

impl Blob {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            source: Source::Memory(bytes.into()),
            filename: None,
            media_type: None,
            sensitive: false,
            content: Arc::default(),
        }
    }

    #[must_use]
    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }

    #[must_use]
    pub fn with_media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    #[must_use]
    pub fn sensitive(mut self) -> Self {
        self.sensitive = true;
        self
    }

    pub fn bytes(&self) -> &[u8] {
        match &self.source {
            Source::Memory(bytes) => bytes,
        }
    }

    pub fn filename(&self) -> Option<&str> {
        self.filename.as_deref()
    }

    pub fn media_type(&self) -> Option<&str> {
        self.media_type.as_deref()
    }

    pub fn is_sensitive(&self) -> bool {
        self.sensitive
    }

    pub fn descriptor(&self) -> BytesDescriptor {
        if self.sensitive {
            return BytesDescriptor::redacted();
        }
        let mut descriptor = self
            .content
            .get_or_init(|| BytesDescriptor::of(self.bytes(), None, None))
            .clone();
        if let BytesDescriptor::Content(content) = &mut descriptor {
            content.media_type.clone_from(&self.media_type);
            content.filename.clone_from(&self.filename);
        }
        descriptor
    }
}

impl fmt::Debug for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Blob")
            .field("len", &self.bytes().len())
            .field("filename", &self.filename)
            .field("media_type", &self.media_type)
            .field("sensitive", &self.sensitive)
            .finish()
    }
}

impl PartialEq for Blob {
    fn eq(&self, other: &Self) -> bool {
        self.sensitive == other.sensitive
            && self.filename == other.filename
            && self.media_type == other.media_type
            && self.bytes() == other.bytes()
    }
}

impl Eq for Blob {}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkflowInputs {
    pub values: BTreeMap<String, Value>,
    pub blobs: BTreeMap<String, Blob>,
}

impl WorkflowInputs {
    pub fn new(values: BTreeMap<String, Value>, blobs: BTreeMap<String, Blob>) -> Self {
        Self { values, blobs }
    }
}

impl From<BTreeMap<String, Value>> for WorkflowInputs {
    fn from(values: BTreeMap<String, Value>) -> Self {
        Self {
            values,
            blobs: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rendered(blob: &Blob) -> Value {
        serde_json::to_value(blob.descriptor()).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn a_descriptor_carries_size_hash_and_metadata_and_never_content() {
        let blob = Blob::new(vec![0xFF, 0xD8, 0xFF])
            .with_filename("cat.jpg")
            .with_media_type("image/jpeg");
        assert_eq!(blob.bytes(), [0xFF, 0xD8, 0xFF]);
        assert_eq!(
            rendered(&blob),
            json!({
                "kind": "bytes",
                "size": 3,
                "sha256": "6e568e1f67fba258184c78181539e5e8fdee447e49bb706fc0ea34fbf12336a5",
                "mediaType": "image/jpeg",
                "filename": "cat.jpg"
            })
        );
        assert_eq!(
            rendered(&Blob::new(Vec::new())),
            json!({
                "kind": "bytes",
                "size": 0,
                "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            })
        );
    }

    #[test]
    fn a_sensitive_blob_renders_the_fixed_marker_wherever_it_is_copied() {
        let blob = Blob::new(b"secret".to_vec())
            .with_filename("key.pem")
            .sensitive();
        let copy = blob.clone();
        assert!(copy.is_sensitive());
        assert_eq!(rendered(&copy), json!({"kind": "bytes", "redacted": true}));
        assert_eq!(copy.bytes(), b"secret");
    }

    #[test]
    fn debug_output_and_metadata_changes_never_expose_or_stale_content() {
        let blob = Blob::new(b"planted-bytes".to_vec());
        let hashed = rendered(&blob);
        let renamed = blob.clone().with_filename("a.bin");
        assert_eq!(rendered(&renamed)["filename"], "a.bin");
        assert_eq!(rendered(&renamed)["sha256"], hashed["sha256"]);
        assert!(hashed.get("filename").is_none());
        let debug = format!("{renamed:?}");
        assert!(!debug.contains("planted"), "{debug}");
        assert!(debug.contains("len: 13"), "{debug}");
        assert_ne!(blob, renamed);
        assert_eq!(blob, Blob::new(b"planted-bytes".to_vec()));
    }

    #[test]
    fn json_inputs_convert_with_no_bytes() {
        let inputs = WorkflowInputs::from(BTreeMap::from([("a".to_owned(), json!(1))]));
        assert!(inputs.blobs.is_empty());
        assert_eq!(inputs.values["a"], json!(1));
    }
}
