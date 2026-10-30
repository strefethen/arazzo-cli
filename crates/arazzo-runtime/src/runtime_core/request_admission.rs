use super::*;

pub(super) fn request_context(
    vars: &VarStore,
    mut context: EvalContext,
    method: &str,
) -> EvalContext {
    for (name, metadata) in &vars.input_provenance {
        if metadata.unresolved {
            context.inputs.remove(name);
        }
    }
    for name in &vars.optional_omissions {
        context.inputs.entry(name.clone()).or_insert(Value::Null);
    }
    let steps = Arc::make_mut(&mut context.steps);
    for (step_id, metadata) in vars.step_provenance.iter() {
        if let Some(outputs) = steps.get_mut(step_id) {
            for (name, metadata) in metadata {
                if metadata.unresolved {
                    outputs.remove(name);
                }
            }
        }
    }
    for (workflow_id, metadata) in &vars.workflow_provenance {
        if let Some(workflow) = context.workflows.get_mut(workflow_id) {
            for (name, metadata) in &metadata.inputs {
                if metadata.unresolved {
                    workflow.inputs.remove(name);
                }
            }
            for (name, metadata) in &metadata.outputs {
                if metadata.unresolved {
                    workflow.outputs.remove(name);
                }
            }
            for name in &metadata.optional_omissions {
                workflow.inputs.entry(name.clone()).or_insert(Value::Null);
            }
        }
    }
    context.method = Some(method.to_owned());
    context
}

pub(super) struct RequestAdmission<'a> {
    step_id: &'a str,
    vars: &'a VarStore,
    presentation: Option<dry_run::RequestPresentation<'a>>,
}

impl<'a> RequestAdmission<'a> {
    pub(super) fn new(step_id: &'a str, vars: &'a VarStore) -> Self {
        Self {
            step_id,
            vars,
            presentation: None,
        }
    }

    pub(super) fn with_presentation(mut self, workflow_id: &'a str) -> Self {
        self.presentation = Some(dry_run::RequestPresentation::new(workflow_id, self.step_id));
        self
    }

    pub(super) fn project(
        &self,
        result: &payload::ResolvedValueSource,
        location: &str,
    ) -> Result<Value, RuntimeError> {
        self.check(result, location)?;
        Ok(self.presentation.as_ref().map_or_else(
            || result.value.clone(),
            |presentation| presentation.project(self.vars, result, location),
        ))
    }

    pub(super) fn presentation_warnings(&self, step: &Step) -> Vec<String> {
        self.presentation
            .as_ref()
            .map_or_else(Vec::new, |presentation| presentation.warnings(step))
    }

    pub(super) fn parameter_location(index: usize, parameter: &Parameter) -> String {
        let location = match parameter.in_ {
            Some(ParamLocation::Path) => "path",
            Some(ParamLocation::Query) => "query",
            Some(ParamLocation::Querystring) => "querystring",
            Some(ParamLocation::Header) => "header",
            Some(ParamLocation::Cookie) => "cookie",
            None => "unspecified",
        };
        format!("parameters[{index}]({location}:{})", parameter.name)
    }

    pub(super) fn check(
        &self,
        result: &payload::ResolvedValueSource,
        location: &str,
    ) -> Result<(), RuntimeError> {
        use payload::ValueResolutionFailureReason;
        let outputs = ProvenancedValues::default();
        if !self
            .vars
            .value_provenance(result, &outputs, None)
            .unresolved
        {
            return Ok(());
        }
        for failure in &result.failures {
            let occurrence = result.expressions.iter().find(|occurrence| {
                occurrence.pointer == failure.pointer
                    && occurrence.resolution.expression == failure.source
            });
            let source = match failure.reason {
                ValueResolutionFailureReason::ExpressionSyntax => "invalid expression syntax",
                ValueResolutionFailureReason::SelectorHardFailure => "Selector value",
                ValueResolutionFailureReason::ExpressionMissing
                | ValueResolutionFailureReason::SelectorContextUnresolved => {
                    if let Some(occurrence) = occurrence {
                        if !self.occurrence_unresolved(&occurrence.resolution) {
                            continue;
                        }
                        occurrence.resolution.expression.as_str()
                    } else {
                        "Selector context"
                    }
                }
            };
            return Err(self.refusal(location, &failure.pointer, source));
        }
        for occurrence in &result.expressions {
            if self.occurrence_unresolved(&occurrence.resolution) {
                return Err(self.refusal(
                    location,
                    &occurrence.pointer,
                    &occurrence.resolution.expression,
                ));
            }
        }
        Ok(())
    }

    fn occurrence_unresolved(&self, occurrence: &arazzo_expr::ExpressionResolution) -> bool {
        let value = arazzo_expr::ValueResolution {
            value: Value::Null,
            warnings: Vec::new(),
            expressions: vec![occurrence.clone()],
            syntax_error: false,
        };
        self.vars
            .expression_provenance(&value, &ProvenancedValues::default(), None)
            .unresolved
    }

    fn refusal(&self, location: &str, pointer: &str, source: &str) -> RuntimeError {
        RuntimeError::new(
            RuntimeErrorKind::UnresolvedRequestValue,
            format!(
                "step {:?}: {location}{pointer}: {source} did not resolve; request not sent",
                self.step_id
            ),
        )
    }
}
