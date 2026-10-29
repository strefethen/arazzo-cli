//! Borrowed Workflow-to-Step parameter inheritance, independent of target kind.

use std::collections::HashSet;

use crate::{Parameter, Workflow};

/// The source declaration of an effective parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveParameterOrigin {
    /// A declaration in [`Workflow::parameters`].
    Workflow { parameter_index: usize },
    /// A declaration in the indexed Step's parameter list.
    Step {
        step_index: usize,
        parameter_index: usize,
    },
}

/// A parameter borrowed from its original declaration, with declaration indexes.
#[derive(Clone, Copy, Debug)]
pub struct EffectiveParameterRef<'a> {
    pub parameter: &'a Parameter,
    pub origin: EffectiveParameterOrigin,
}

/// Applies Workflow parameter inheritance and exact Step overrides.
///
/// Identity is the case-sensitive `(name, in_)` pair, including an absent
/// location. The returned order is a repository choice: non-overridden Workflow
/// declarations in source order, followed by every Step declaration in source
/// order. Duplicates remain visible for validation, but a Step identity excludes
/// every matching inherited declaration.
///
/// All target kinds use the same overlay. Values and component references are
/// borrowed unchanged; this function does not validate or resolve declarations.
/// An invalid Step index returns `None`; a valid empty view returns `Some([])`.
pub fn effective_step_parameters(
    workflow: &Workflow,
    step_index: usize,
) -> Option<Vec<EffectiveParameterRef<'_>>> {
    let step = workflow.steps.get(step_index)?;
    let step_identities: HashSet<_> = step
        .parameters
        .iter()
        .map(|parameter| (parameter.name.as_str(), parameter.in_))
        .collect();

    let inherited = workflow
        .parameters
        .iter()
        .enumerate()
        .filter(|(_, parameter)| {
            !step_identities.contains(&(parameter.name.as_str(), parameter.in_))
        })
        .map(|(parameter_index, parameter)| EffectiveParameterRef {
            parameter,
            origin: EffectiveParameterOrigin::Workflow { parameter_index },
        });
    let declared = step
        .parameters
        .iter()
        .enumerate()
        .map(|(parameter_index, parameter)| EffectiveParameterRef {
            parameter,
            origin: EffectiveParameterOrigin::Step {
                step_index,
                parameter_index,
            },
        });

    Some(inherited.chain(declared).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Step, ValueSource};

    #[test]
    fn view_borrows_opaque_declarations_without_mutation_or_resolution() {
        let workflow = Workflow {
            parameters: vec![Parameter {
                name: "unresolved".into(),
                reference: "$components.parameters.missing".into(),
                value: ValueSource::Literal(serde_yaml_ng::Value::String("$not.parsed".into())),
                ..Parameter::default()
            }],
            steps: vec![Step::default()],
            ..Workflow::default()
        };
        let before = workflow.clone();
        let Some(effective) = effective_step_parameters(&workflow, 0) else {
            panic!("Step zero exists");
        };

        assert_eq!(effective.len(), 1);
        assert!(std::ptr::eq(
            effective[0].parameter,
            &workflow.parameters[0]
        ));
        assert_eq!(
            effective[0].origin,
            EffectiveParameterOrigin::Workflow { parameter_index: 0 }
        );
        assert_eq!(workflow, before);
    }
}
