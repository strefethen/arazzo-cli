use std::cell::RefCell;

use arazzo_expr::StoredValueReference;

use super::value_provenance::SkippedValueSite;
use super::*;

pub(super) struct RequestPresentation<'a> {
    workflow_id: &'a str,
    step_id: &'a str,
    warnings: RefCell<BTreeMap<String, Vec<String>>>,
}

impl<'a> RequestPresentation<'a> {
    pub(super) fn new(workflow_id: &'a str, step_id: &'a str) -> Self {
        Self {
            workflow_id,
            step_id,
            warnings: RefCell::new(BTreeMap::new()),
        }
    }

    pub(super) fn project(
        &self,
        vars: &VarStore,
        result: &payload::ResolvedValueSource,
        location: &str,
    ) -> Value {
        let mut value = result.value.clone();
        let mut warnings = Vec::new();
        for occurrence in &result.expressions {
            if !occurrence.direct
                || !matches!(
                    occurrence.resolution.reference,
                    Some(
                        StoredValueReference::Input { pointer: None, .. }
                            | StoredValueReference::StepOutput { pointer: None, .. }
                            | StoredValueReference::WorkflowOutput { pointer: None, .. }
                    )
                )
            {
                continue;
            }
            let resolution = arazzo_expr::ValueResolution {
                value: Value::Null,
                warnings: Vec::new(),
                expressions: vec![occurrence.resolution.clone()],
                syntax_error: false,
            };
            let metadata =
                vars.expression_provenance(&resolution, &ProvenancedValues::default(), None);
            let Some(first) = metadata.skipped.first() else {
                continue;
            };
            let marker = Value::String(format!("<unresolved:{}>", first.source));
            if occurrence.pointer.is_empty() {
                value = marker;
            } else if let Some(leaf) = value.pointer_mut(&occurrence.pointer) {
                *leaf = marker;
            }
            let producer = producer_name(first);
            let pointer = if location == "requestBody.payload" {
                occurrence.pointer.as_str()
            } else {
                ""
            };
            warnings.push(format!("dry-run unresolved {}/{}/{location}{pointer}: {producer} depends on {}; transport was skipped", self.workflow_id, self.step_id, first.source));
        }
        self.warnings
            .borrow_mut()
            .entry(location.to_owned())
            .or_default()
            .extend(warnings);
        value
    }

    pub(super) fn warnings(&self, step: &Step) -> Vec<String> {
        let mut warnings = self.warnings.borrow_mut();
        let mut ordered = Vec::new();
        for (index, parameter) in step.parameters.iter().enumerate() {
            ordered.extend(
                warnings
                    .remove(&request_admission::RequestAdmission::parameter_location(
                        index, parameter,
                    ))
                    .unwrap_or_default(),
            );
        }
        ordered.extend(warnings.remove("requestBody.payload").unwrap_or_default());
        if let Some(body) = &step.request_body {
            for index in 0..body.replacements.len() {
                ordered.extend(
                    warnings
                        .remove(&format!("requestBody.replacements[{index}].value"))
                        .unwrap_or_default(),
                );
            }
        }
        ordered
    }
}

fn producer_name(origin: &value_provenance::SkippedOrigin) -> String {
    let site = match &origin.site {
        SkippedValueSite::Output(name) => format!("outputs.{name}"),
        SkippedValueSite::ActionParameter(name) => format!("action.parameters.{name}"),
    };
    format!("{}/{}.{site}", origin.workflow_id, origin.step_id)
}

pub(super) fn output_warnings(
    workflow_id: &str,
    step_id: &str,
    name: &str,
    output: &OutputValue,
    metadata: &ValueProvenance,
) -> Vec<String> {
    if !matches!(output, OutputValue::RuntimeExpression(_)) {
        return Vec::new();
    }
    metadata
        .skipped
        .iter()
        .filter(|origin| {
            origin.workflow_id == workflow_id
                && origin.step_id == step_id
                && origin.site == SkippedValueSite::Output(name.to_owned())
        })
        .map(|origin| {
            format!(
                "dry-run unresolved output {}: {} requires a response; transport was skipped",
                producer_name(origin),
                origin.source
            )
        })
        .collect()
}
