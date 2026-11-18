use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

use arazzo_expr::{BlobSlot, ExpressionEvaluator, ValueUnavailable};
use serde_json::Value;

use super::{BytesDescriptor, VarStore};

pub(crate) const BYTES_SCOPE_MARKER: &str = "bytes (not viewable until descriptor support)";

pub(super) fn mark_bytes(values: &mut BTreeMap<String, Value>, blobs: &BTreeMap<String, Blob>) {
    for name in blobs.keys() {
        values.insert(name.clone(), Value::String(BYTES_SCOPE_MARKER.to_owned()));
    }
}

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

/// A workflow's outputs: JSON values, and bytes keyed like them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkflowOutputs {
    pub values: BTreeMap<String, Value>,
    pub blobs: BTreeMap<String, Blob>,
}

impl WorkflowOutputs {
    /// Every output as it is serialized: a JSON value, or a blob's descriptor.
    pub fn presented(&self) -> BTreeMap<String, Value> {
        presented(&self.values, &self.blobs)
    }
}

impl From<super::ProvenancedValues> for WorkflowOutputs {
    fn from(outputs: super::ProvenancedValues) -> Self {
        Self {
            values: outputs.values,
            blobs: outputs.blobs,
        }
    }
}

pub(crate) fn presented(
    values: &BTreeMap<String, Value>,
    blobs: &BTreeMap<String, Blob>,
) -> BTreeMap<String, Value> {
    let mut presented = values.clone();
    for (name, blob) in blobs {
        presented.insert(name.clone(), blob.descriptor().to_value());
    }
    presented
}

pub(crate) fn descriptors(blobs: &BTreeMap<String, Blob>) -> BTreeMap<String, BytesDescriptor> {
    blobs
        .iter()
        .map(|(name, blob)| (name.clone(), blob.descriptor()))
        .collect()
}

/// One stored slot: a JSON value, or bytes, which never become one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Slot {
    Value(Value),
    Blob(Blob),
}

impl From<Value> for Slot {
    fn from(value: Value) -> Self {
        Self::Value(value)
    }
}

/// Where a whole-value reference to a bytes slot finds its bytes.
#[derive(Clone, Copy)]
pub(super) struct BlobSources<'a> {
    pub(super) vars: &'a VarStore,
    pub(super) request_body: Option<&'a Blob>,
    pub(super) response_body: Option<&'a Blob>,
    pub(super) outputs: Option<&'a BTreeMap<String, Blob>>,
}

impl<'a> BlobSources<'a> {
    pub(super) fn new(vars: &'a VarStore) -> Self {
        Self {
            vars,
            request_body: None,
            response_body: None,
            outputs: None,
        }
    }

    /// The bytes `expression` names when it is a whole reference to a bytes
    /// slot; `None` for every other expression, which resolves as a value.
    pub(super) fn whole(
        &self,
        eval: &ExpressionEvaluator,
        expression: &str,
    ) -> Result<Option<Blob>, ValueUnavailable> {
        let Some(slot) = eval.blob_slot(expression) else {
            return Ok(None);
        };
        let blob = match &slot {
            BlobSlot::RequestBody => self.request_body.cloned(),
            BlobSlot::ResponseBody => self.response_body.cloned(),
            BlobSlot::Output { name } => self.outputs.and_then(|blobs| blobs.get(name)).cloned(),
            slot => self.vars.blob(slot),
        };
        blob.map(Some).ok_or_else(|| ValueUnavailable {
            expression: expression.to_owned(),
            reason: format!("{slot} is bytes that are not held at this site"),
        })
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

    fn step_slot(name: &str) -> BlobSlot {
        BlobSlot::StepOutput {
            step_id: "keep".to_owned(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn every_copy_of_a_step_store_keeps_its_bytes_slots() {
        let photo = Blob::new(b"jpeg".to_vec());
        let mut outputs = super::super::ProvenancedValues::default();
        outputs.insert(
            "photo".to_owned(),
            Slot::Blob(photo.clone()),
            super::super::ValueProvenance::default(),
        );
        let mut vars = VarStore::default();
        vars.merge_step_outputs("keep", outputs.clone());
        let readable = |vars: &VarStore| {
            let named = vars
                .blob_slots()
                .steps
                .get("keep")
                .is_some_and(|names| names.contains("photo"));
            (named, vars.blob(&step_slot("photo")))
        };
        assert_eq!(readable(&vars), (true, Some(photo.clone())), "merge");
        assert_eq!(
            readable(&vars.clone()),
            (true, Some(photo.clone())),
            "clone"
        );

        let mut caption = super::super::ProvenancedValues::default();
        caption.insert(
            "caption".to_owned(),
            Value::from("cat"),
            super::super::ValueProvenance::default(),
        );
        vars.merge_step_outputs("keep", caption);
        assert_eq!(readable(&vars), (true, Some(photo.clone())), "second merge");
        assert_eq!(vars.step_values("keep").values["caption"], "cat");

        let previous = vars.replace_step_outputs("keep", BTreeMap::new());
        assert_eq!(readable(&vars), (false, None), "replaced");
        vars.restore_step_outputs("keep", previous);
        assert_eq!(readable(&vars), (true, Some(photo.clone())), "restored");

        let mut renamed = super::super::ProvenancedValues::default();
        renamed.insert(
            "photo".to_owned(),
            Value::from("text now"),
            super::super::ValueProvenance::default(),
        );
        vars.merge_step_outputs("keep", renamed);
        assert_eq!(readable(&vars), (false, None), "a value replaces the bytes");

        vars.merge_step_outputs("keep", outputs.clone());
        let scopes = vars.debug_scopes();
        assert_eq!(scopes.steps["keep"]["photo"], BYTES_SCOPE_MARKER);
        assert_eq!(scopes.steps["keep"]["caption"], "cat");

        vars.register_workflow_values(
            "child",
            super::super::ProvenancedValues::default(),
            outputs,
            std::collections::BTreeSet::new(),
        );
        assert!(vars.blob_slots().workflow_outputs["child"].contains("photo"));
        vars.register_workflow_values(
            "child",
            super::super::ProvenancedValues::default(),
            super::super::ProvenancedValues::default(),
            std::collections::BTreeSet::new(),
        );
        assert!(!vars.blob_slots().workflow_outputs.contains_key("child"));
    }

    #[test]
    fn json_inputs_convert_with_no_bytes() {
        let inputs = WorkflowInputs::from(BTreeMap::from([("a".to_owned(), json!(1))]));
        assert!(inputs.blobs.is_empty());
        assert_eq!(inputs.values["a"], json!(1));
    }
}
