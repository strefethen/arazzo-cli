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

    start < spec.workflows.len()
        && graph_has_cycle(
            spec.workflows.len(),
            start..=start,
            |index| spec.workflows[index].depends_on.as_slice(),
            |dependency| match classify_workflow_dependency(dependency) {
                WorkflowDependency::Local(workflow_id) => positions.get(workflow_id).copied(),
                _ => None,
            },
        )
}

pub(crate) fn local_step_depends_on_has_cycle(workflow: &Workflow) -> bool {
    let positions = workflow
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.step_id.as_str(), index))
        .collect::<HashMap<_, _>>();

    graph_has_cycle(
        workflow.steps.len(),
        0..workflow.steps.len(),
        |index| workflow.steps[index].depends_on.as_slice(),
        |dependency| match classify_step_dependency(dependency) {
            StepDependency::Local(step_id) => positions.get(step_id).copied(),
            _ => None,
        },
    )
}

/// Depth-first cycle search over `node_count` nodes with an explicit work
/// stack, so stack use does not grow with chain length. Nodes are unvisited,
/// in progress, or done; reaching an in-progress node is a cycle. `resolve`
/// maps a `dependsOn` entry to a local node, or `None` to skip it.
fn graph_has_cycle<'a>(
    node_count: usize,
    starts: impl IntoIterator<Item = usize>,
    depends_on: impl Fn(usize) -> &'a [String],
    resolve: impl Fn(&str) -> Option<usize>,
) -> bool {
    const IN_PROGRESS: u8 = 1;
    const DONE: u8 = 2;

    let mut states = vec![0_u8; node_count];
    // Each frame is a node and the index of its next `dependsOn` entry.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for start in starts {
        match states[start] {
            IN_PROGRESS => return true,
            DONE => continue,
            _ => {}
        }
        states[start] = IN_PROGRESS;
        stack.push((start, 0));
        while let Some(&(index, cursor)) = stack.last() {
            let dependencies = depends_on(index);
            let Some(dependency) = dependencies.get(cursor) else {
                states[index] = DONE;
                stack.pop();
                continue;
            };
            if let Some(frame) = stack.last_mut() {
                frame.1 += 1;
            }
            let Some(target) = resolve(dependency) else {
                continue;
            };
            match states[target] {
                IN_PROGRESS => return true,
                DONE => {}
                _ => {
                    states[target] = IN_PROGRESS;
                    stack.push((target, 0));
                }
            }
        }
    }
    false
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
