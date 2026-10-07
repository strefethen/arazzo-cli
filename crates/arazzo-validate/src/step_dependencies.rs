//! Step and Workflow `dependsOn` support: Step dependency classification and
//! local dependency cycle detection for Step and Workflow `dependsOn` lists.
//!
//! The crate root keeps the call sites in `collect_diagnostics` and delegates
//! here.

use std::collections::HashMap;

use arazzo_spec::{classify_workflow_dependency, ArazzoSpec, Workflow, WorkflowDependency};

pub(crate) fn local_workflow_depends_on_has_cycle(spec: &ArazzoSpec, start: usize) -> bool {
    let positions = spec
        .workflows
        .iter()
        .enumerate()
        .map(|(index, workflow)| (workflow.workflow_id.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut states = vec![0_u8; spec.workflows.len()];

    fn visit(
        index: usize,
        spec: &ArazzoSpec,
        positions: &HashMap<&str, usize>,
        states: &mut [u8],
    ) -> bool {
        if states[index] == 1 {
            return true;
        }
        if states[index] == 2 {
            return false;
        }
        states[index] = 1;
        for dependency in &spec.workflows[index].depends_on {
            let WorkflowDependency::Local(workflow_id) = classify_workflow_dependency(dependency)
            else {
                continue;
            };
            let Some(&dependency_index) = positions.get(workflow_id) else {
                continue;
            };
            if visit(dependency_index, spec, positions, states) {
                return true;
            }
        }
        states[index] = 2;
        false
    }

    start < spec.workflows.len() && visit(start, spec, &positions, &mut states)
}

pub(crate) fn local_step_depends_on_has_cycle(workflow: &Workflow) -> bool {
    let positions = workflow
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.step_id.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut states = vec![0_u8; workflow.steps.len()];

    fn visit(
        index: usize,
        workflow: &Workflow,
        positions: &HashMap<&str, usize>,
        states: &mut [u8],
    ) -> bool {
        if states[index] == 1 {
            return true;
        }
        if states[index] == 2 {
            return false;
        }
        states[index] = 1;
        for dependency in &workflow.steps[index].depends_on {
            let StepDependency::Local(step_id) = classify_step_dependency(dependency) else {
                continue;
            };
            let Some(&dependency_index) = positions.get(step_id) else {
                continue;
            };
            if visit(dependency_index, workflow, positions, states) {
                return true;
            }
        }
        states[index] = 2;
        false
    }

    (0..workflow.steps.len()).any(|index| visit(index, workflow, &positions, &mut states))
}

pub(crate) enum StepDependency<'a> {
    Local(&'a str),
    CrossWorkflow,
    ExternalSource,
    Invalid,
}

pub(crate) fn classify_step_dependency(value: &str) -> StepDependency<'_> {
    if value.is_empty() {
        return StepDependency::Invalid;
    }
    if !value.starts_with('$') {
        return StepDependency::Local(value);
    }

    let parts = value.split('.').collect::<Vec<_>>();
    match parts.as_slice() {
        ["$workflows", workflow_id, "steps", step_id]
            if !workflow_id.is_empty() && !step_id.is_empty() =>
        {
            StepDependency::CrossWorkflow
        }
        ["$sourceDescriptions", source_name, workflow_id, "steps", step_id]
            if !source_name.is_empty() && !workflow_id.is_empty() && !step_id.is_empty() =>
        {
            StepDependency::ExternalSource
        }
        _ => StepDependency::Invalid,
    }
}
