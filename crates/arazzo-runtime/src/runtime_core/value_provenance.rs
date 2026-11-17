use super::payload::{ResolvedValueSource, ValueResolutionFailureReason};
use super::*;
use arazzo_expr::{RuntimeExpressionNamespace, StoredValueReference, ValueResolution};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProvenancedValues {
    pub(crate) values: BTreeMap<String, Value>,
    pub(super) provenance: BTreeMap<String, ValueProvenance>,
    pub(super) blobs: BTreeMap<String, Blob>,
}

impl From<BTreeMap<String, Value>> for ProvenancedValues {
    fn from(values: BTreeMap<String, Value>) -> Self {
        Self {
            values,
            provenance: BTreeMap::new(),
            blobs: BTreeMap::new(),
        }
    }
}

impl From<WorkflowInputs> for ProvenancedValues {
    fn from(inputs: WorkflowInputs) -> Self {
        Self {
            values: inputs.values,
            provenance: BTreeMap::new(),
            blobs: inputs.blobs,
        }
    }
}

impl ProvenancedValues {
    pub(super) fn insert(
        &mut self,
        name: String,
        slot: impl Into<Slot>,
        metadata: ValueProvenance,
    ) {
        self.provenance.remove(&name);
        match slot.into() {
            Slot::Value(value) => {
                self.blobs.remove(&name);
                self.values.insert(name.clone(), value);
            }
            Slot::Blob(blob) => {
                self.values.remove(&name);
                self.blobs.insert(name.clone(), blob);
            }
        }
        if metadata.unresolved || !metadata.skipped.is_empty() {
            self.provenance.insert(name, metadata);
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.values.is_empty() && self.blobs.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ValueProvenance {
    pub(super) unresolved: bool,
    pub(super) skipped: Vec<SkippedOrigin>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct WorkflowProvenance {
    pub(super) inputs: BTreeMap<String, ValueProvenance>,
    pub(super) outputs: BTreeMap<String, ValueProvenance>,
    pub(super) optional_omissions: BTreeSet<String>,
}

pub(super) fn optional_input_omissions(
    schema: Option<&arazzo_spec::SchemaObject>,
    inputs: &BTreeMap<String, Value>,
) -> BTreeSet<String> {
    schema
        .map(|schema| {
            schema
                .properties
                .keys()
                .filter(|name| !schema.required.contains(name) && !inputs.contains_key(*name))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SkippedOrigin {
    pub(super) workflow_id: String,
    pub(super) step_id: String,
    pub(super) site: SkippedValueSite,
    pub(super) source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SkippedValueSite {
    Output(String),
    ActionParameter(String),
}

#[derive(Clone, Copy)]
pub(super) enum ValueProducerSite<'a> {
    Output(&'a str),
    ActionParameter(&'a str),
}

impl ValueProducerSite<'_> {
    pub(super) fn to_owned(self) -> SkippedValueSite {
        match self {
            Self::Output(name) => SkippedValueSite::Output(name.to_owned()),
            Self::ActionParameter(name) => SkippedValueSite::ActionParameter(name.to_owned()),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ValueProducerTransport {
    Http,
    Message,
}

impl ValueProducerTransport {
    fn includes(self, namespace: RuntimeExpressionNamespace) -> bool {
        match self {
            Self::Http => matches!(
                namespace,
                RuntimeExpressionNamespace::Response | RuntimeExpressionNamespace::StatusCode
            ),
            Self::Message => namespace == RuntimeExpressionNamespace::Message,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ValueProducer<'a> {
    pub(super) workflow_id: &'a str,
    pub(super) step_id: &'a str,
    pub(super) site: ValueProducerSite<'a>,
    pub(super) transport: ValueProducerTransport,
}

impl ValueProvenance {
    fn merge(&mut self, other: &Self) {
        self.unresolved |= other.unresolved;
        for origin in &other.skipped {
            if !self.skipped.contains(origin) {
                self.skipped.push(origin.clone());
            }
        }
    }
}

impl VarStore {
    pub(super) fn value_provenance(
        &self,
        result: &ResolvedValueSource,
        outputs: &ProvenancedValues,
        producer: Option<ValueProducer<'_>>,
    ) -> ValueProvenance {
        self.join_provenance(
            result
                .expressions
                .iter()
                .map(|item| (item.pointer.as_str(), &item.resolution)),
            result.failures.iter().map(|failure| {
                (
                    failure.pointer.as_str(),
                    failure.source.as_str(),
                    failure.reason,
                )
            }),
            outputs,
            producer,
        )
    }

    pub(super) fn expression_provenance(
        &self,
        result: &ValueResolution,
        outputs: &ProvenancedValues,
        producer: Option<ValueProducer<'_>>,
    ) -> ValueProvenance {
        let failures =
            result
                .syntax_error
                .then_some(("", "", ValueResolutionFailureReason::ExpressionSyntax));
        self.join_provenance(
            result.expressions.iter().map(|item| ("", item)),
            failures.into_iter(),
            outputs,
            producer,
        )
    }

    fn join_provenance<'a>(
        &self,
        expressions: impl Iterator<Item = (&'a str, &'a arazzo_expr::ExpressionResolution)>,
        failures: impl Iterator<Item = (&'a str, &'a str, ValueResolutionFailureReason)>,
        outputs: &ProvenancedValues,
        producer: Option<ValueProducer<'_>>,
    ) -> ValueProvenance {
        let mut result = ValueProvenance::default();
        let mut skipped_occurrences = Vec::new();
        for (pointer, occurrence) in expressions {
            let mut metadata = occurrence
                .reference
                .as_ref()
                .and_then(|reference| self.stored_provenance(reference, outputs))
                .cloned()
                .unwrap_or_default();
            if let Some(producer) = producer {
                if producer.transport.includes(occurrence.namespace) {
                    metadata.skipped.push(SkippedOrigin {
                        workflow_id: producer.workflow_id.to_owned(),
                        step_id: producer.step_id.to_owned(),
                        site: producer.site.to_owned(),
                        source: occurrence.expression.clone(),
                    });
                }
            }
            if occurrence.missing {
                if metadata.skipped.is_empty() || metadata.unresolved {
                    metadata.unresolved = true;
                } else {
                    skipped_occurrences.push((pointer, occurrence.expression.as_str()));
                }
            }
            result.merge(&metadata);
        }
        for (pointer, source, reason) in failures {
            if matches!(
                reason,
                ValueResolutionFailureReason::ExpressionMissing
                    | ValueResolutionFailureReason::SelectorContextUnresolved
            ) && skipped_occurrences.contains(&(pointer, source))
            {
                continue;
            }
            result.unresolved = true;
        }
        result
    }

    fn stored_provenance<'a>(
        &'a self,
        reference: &StoredValueReference,
        outputs: &'a ProvenancedValues,
    ) -> Option<&'a ValueProvenance> {
        match reference {
            StoredValueReference::Input { name, .. } => self.input_provenance.get(name),
            StoredValueReference::Output { name, .. } => outputs.provenance.get(name),
            StoredValueReference::StepOutput { step_id, name, .. } => {
                self.step_provenance.get(step_id)?.get(name)
            }
            StoredValueReference::WorkflowInput {
                workflow_id, name, ..
            } => self.workflow_provenance.get(workflow_id)?.inputs.get(name),
            StoredValueReference::WorkflowOutput {
                workflow_id, name, ..
            } => self.workflow_provenance.get(workflow_id)?.outputs.get(name),
        }
    }
}
