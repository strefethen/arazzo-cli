use super::*;

#[allow(dead_code)]
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
