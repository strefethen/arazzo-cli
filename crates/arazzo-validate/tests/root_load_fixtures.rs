#[path = "../benches/support/root_load.rs"]
mod support;

use std::collections::BTreeSet;

use arazzo_spec::{ArazzoSpec, StepTarget};
use arazzo_validate::{
    parse_bytes_with_diagnostics, Diagnostic, Error, Severity, ValidationErrorKind,
};
use serde_yaml_ng::Value;
use support::{Fixtures, ParseResult};

#[test]
fn committed_fixtures_pass_complete_preflight() -> Result<(), String> {
    let fixtures = Fixtures::load()?;
    fixtures.preflight()?;
    assert_eq!(fixtures.roots.map(|case| case.steps), [10, 100, 1000]);
    Ok(())
}

#[test]
fn positive_roots_have_exact_models_identities_order_operations_and_caps() -> Result<(), String> {
    let fixtures = Fixtures::load()?;
    for case in &fixtures.roots {
        assert!(case.bytes.len() <= support::ROOT_BYTE_CAP);
        let result = parse_bytes_with_diagnostics(&case.bytes);
        case.verify(&result)?;
        let (spec, diagnostics) = result.map_err(|error| error.to_string())?;
        assert!(diagnostics.is_empty());
        assert_eq!(spec.arazzo, "1.1.0");
        assert_eq!(spec.info.title, "Root load benchmark");
        assert_eq!(spec.info.version, "1.0.0");
        assert_eq!(spec.source_descriptions.len(), 1);
        assert_eq!(spec.source_descriptions[0].name, "api");
        assert_eq!(spec.source_descriptions[0].url, "openapi.yaml");
        assert_eq!(spec.workflows.len(), 1);
        let workflow = &spec.workflows[0];
        assert_eq!(workflow.workflow_id, "root_load");
        assert_eq!(workflow.steps.len(), case.steps);
        let mut ids = BTreeSet::new();
        for (index, step) in workflow.steps.iter().enumerate() {
            assert_eq!(step.step_id, format!("step_{index:04}"));
            assert!(ids.insert(&step.step_id));
            assert_eq!(
                step.target,
                Some(StepTarget::OperationId(format!("op_{:02}", index % 10)))
            );
        }
        support::verify_operation_resolution(&fixtures.source, &spec)?;
    }
    Ok(())
}

#[test]
fn source_has_exact_shape_unique_operations_responses_and_cap() -> Result<(), String> {
    let fixtures = Fixtures::load()?;
    assert!(fixtures.source_bytes.len() <= support::SOURCE_BYTE_CAP);
    assert_eq!(
        support::verify_source(&fixtures.source_bytes)?,
        fixtures.source
    );
    let paths = fixtures.source["paths"]
        .as_mapping()
        .ok_or("source paths must be a mapping")?;
    assert_eq!(paths.len(), 10);
    let mut operations = BTreeSet::new();
    for index in 0..10 {
        let path = format!("/op_{index:02}");
        let operation = &fixtures.source["paths"][&path]["get"];
        assert_eq!(operation["operationId"], format!("op_{index:02}"));
        assert!(operations.insert(
            operation["operationId"]
                .as_str()
                .ok_or("missing operation ID")?
        ));
        assert_eq!(operation["responses"]["200"]["description"], "OK");
    }
    Ok(())
}

#[test]
fn negative_controls_have_distinct_exact_failure_oracles() -> Result<(), String> {
    let fixtures = Fixtures::load()?;
    let malformed = parse_bytes_with_diagnostics(&fixtures.malformed);
    assert!(matches!(malformed, Err(Error::ParseYaml(_))));
    support::verify_malformed(&malformed)?;
    assert!(support::verify_duplicate(&malformed).is_err());

    let unvalidated = arazzo_spec::parse_unvalidated_bytes(&fixtures.duplicate)
        .map_err(|error| error.to_string())?;
    assert_eq!(unvalidated.workflows[0].steps.len(), 2);
    assert!(unvalidated.workflows[0]
        .steps
        .iter()
        .all(|step| step.step_id == "step_0000"));
    let duplicate = parse_bytes_with_diagnostics(&fixtures.duplicate);
    support::verify_duplicate(&duplicate)?;
    assert!(support::verify_malformed(&duplicate).is_err());
    if let Err(Error::Validation(report)) = duplicate {
        assert!(report.warnings.is_empty());
        assert_eq!(report.errors.len(), 1);
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::DuplicateIdentifier
        );
        assert_eq!(report.errors[0].severity, Severity::Error);
        assert_eq!(report.errors[0].path, support::DUPLICATE_PATH);
    } else {
        return Err("duplicate fixture did not produce a validation report".into());
    }
    Ok(())
}

#[test]
fn full_model_oracle_rejects_incorrect_successes() -> Result<(), String> {
    let fixtures = Fixtures::load()?;
    let case = &fixtures.roots[0];
    let correct = parse_bytes_with_diagnostics(&case.bytes).map_err(|error| error.to_string())?;
    let perturbations: [fn(&mut ArazzoSpec); 9] = [
        |spec| spec.workflows[0].workflow_id = "wrong_workflow".into(),
        |spec| spec.workflows[0].steps[0].step_id = "wrong_step".into(),
        |spec| spec.workflows[0].steps[0].target = Some(StepTarget::OperationId("op_01".into())),
        |spec| spec.workflows[0].steps.swap(0, 1),
        |spec| {
            spec.workflows[0].steps.pop();
        },
        |spec| spec.info.title = "wrong_title".into(),
        |spec| spec.arazzo = "1.0.1".into(),
        |spec| spec.source_descriptions[0].url = "wrong.yaml".into(),
        |spec| spec.workflows[0].steps[0].description = "unexpected optional field".into(),
    ];
    for (index, perturb) in perturbations.into_iter().enumerate() {
        let mut incorrect = correct.clone();
        perturb(&mut incorrect.0);
        assert!(case.verify(&Ok(incorrect)).is_err(), "mutation {index}");
    }
    let mut warned = correct.clone();
    warned.1.push(Diagnostic::warning(
        ValidationErrorKind::UnknownField,
        "workflow root_load",
        "unexpected diagnostic",
    ));
    assert!(case.verify(&Ok(warned)).is_err());
    assert!(support::verify_malformed(&Ok(correct.clone())).is_err());
    assert!(support::verify_duplicate(&Ok(correct)).is_err());
    assert!(case
        .verify(&parse_bytes_with_diagnostics(&fixtures.malformed))
        .is_err());
    Ok(())
}

#[test]
fn duplicate_oracle_rejects_incorrect_error_reports() -> Result<(), String> {
    let fixtures = Fixtures::load()?;
    let Err(Error::Validation(correct)) = parse_bytes_with_diagnostics(&fixtures.duplicate) else {
        return Err("missing duplicate validation report".into());
    };
    let mut reports = Vec::new();
    let mut wrong = correct.clone();
    wrong.errors[0].kind = ValidationErrorKind::MissingRequiredField;
    reports.push(wrong);
    let mut wrong = correct.clone();
    wrong.errors[0].severity = Severity::Warning;
    reports.push(wrong);
    let mut wrong = correct.clone();
    wrong.errors[0].path = "wrong path".into();
    reports.push(wrong);
    let mut wrong = correct.clone();
    wrong.errors.clear();
    reports.push(wrong);
    let mut wrong = correct.clone();
    wrong.errors.push(correct.errors[0].clone());
    reports.push(wrong);
    let mut wrong = correct.clone();
    wrong.warnings.push(Diagnostic::warning(
        ValidationErrorKind::UnknownField,
        "root",
        "unexpected warning",
    ));
    reports.push(wrong);
    for report in reports {
        let result: ParseResult = Err(Error::Validation(report));
        assert!(support::verify_duplicate(&result).is_err());
    }
    Ok(())
}

#[test]
fn source_oracles_reject_changed_shape_duplicate_and_unresolved_operations() -> Result<(), String> {
    let fixtures = Fixtures::load()?;
    for changed in [
        fixtures
            .source_bytes
            .iter()
            .copied()
            .chain(b"servers: []\n".iter().copied())
            .collect::<Vec<_>>(),
        String::from_utf8(fixtures.source_bytes.clone())
            .map_err(|error| error.to_string())?
            .replace("3.2.0", "3.0.0")
            .into_bytes(),
        String::from_utf8(fixtures.source_bytes.clone())
            .map_err(|error| error.to_string())?
            .replace("operationId: op_09", "operationId: wrong_operation")
            .into_bytes(),
    ] {
        assert!(support::verify_source(&changed).is_err());
    }
    let (mut root, _) = parse_bytes_with_diagnostics(&fixtures.roots[0].bytes)
        .map_err(|error| error.to_string())?;
    root.workflows[0].steps[0].target = Some(StepTarget::OperationId("missing_operation".into()));
    assert!(support::verify_operation_resolution(&fixtures.source, &root).is_err());
    let mut source = fixtures.source.clone();
    source["paths"]["/op_09"]["get"]["operationId"] = Value::String("op_00".into());
    assert!(support::verify_operation_resolution(&source, &root).is_err());
    source["paths"]["/op_09"]["get"]["operationId"] = Value::Null;
    assert!(support::verify_operation_resolution(&source, &root).is_err());
    Ok(())
}

#[test]
fn experimental_caps_accept_boundary_and_reject_oversized_bytes() -> Result<(), String> {
    for (name, cap) in [
        ("steps.yaml", support::ROOT_BYTE_CAP),
        ("openapi.yaml", support::SOURCE_BYTE_CAP),
    ] {
        let mut bytes = vec![b' '; cap];
        support::verify_cap(name, &bytes, cap)?;
        bytes.push(b' ');
        assert!(support::verify_cap(name, &bytes, cap).is_err());
    }
    assert!(support::verify_source(&vec![b' '; support::SOURCE_BYTE_CAP + 1]).is_err());
    Ok(())
}
