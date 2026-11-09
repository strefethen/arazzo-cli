    fn operation_id_document(sources: &[(&str, &str)], operation_id: &str) -> String {
        let sources: String = sources
            .iter()
            .map(|(name, source_type)| {
                format!(
                    "  - name: {name}\n    url: ./{name}.yaml\n    type: {source_type}\n"
                )
            })
            .collect();
        format!(
            "arazzo: \"1.1.0\"\ninfo:\n  title: Routing\n  version: \"1.0.0\"\n\
             sourceDescriptions:\n{sources}workflows:\n  - workflowId: wf\n    steps:\n      \
             - stepId: s\n        operationId: '{operation_id}'\n"
        )
    }

    fn operation_id_routing_diagnostics(
        sources: &[(&str, &str)],
        operation_id: &str,
    ) -> Vec<Diagnostic> {
        let document = operation_id_document(sources, operation_id);
        let diagnostics = match parse_bytes_with_diagnostics(document.as_bytes()) {
            Ok((_, warnings)) => warnings,
            Err(Error::Validation(report)) => report.errors,
            Err(other) => panic!("unexpected error for {operation_id:?}: {other}"),
        };
        diagnostics
            .into_iter()
            .filter(|diagnostic| diagnostic.path.ends_with(".operationId"))
            .collect()
    }

    fn single_routing_error(sources: &[(&str, &str)], operation_id: &str) -> Diagnostic {
        let mut diagnostics = operation_id_routing_diagnostics(sources, operation_id);
        assert_eq!(diagnostics.len(), 1, "{operation_id:?}: {diagnostics:?}");
        let diagnostic = diagnostics.remove(0);
        assert_eq!(diagnostic.severity, Severity::Error, "{diagnostic:?}");
        assert_eq!(diagnostic.path, "workflow \"wf\" > step \"s\".operationId");
        diagnostic
    }

    #[test]
    fn bare_operation_id_with_two_openapi_sources_is_an_error_quoting_the_spec() {
        let diagnostic =
            single_routing_error(&[("pets", "openapi"), ("stores", "openapi")], "getPet");
        assert_eq!(diagnostic.kind, ValidationErrorKind::InvalidStepTarget);
        assert!(
            diagnostic.message.contains(
                "If multiple (non arazzo type) sourceDescriptions are defined, then the \
                 operationId MUST be specified using a Runtime Expression"
            ),
            "{diagnostic:?}"
        );
        assert!(diagnostic.message.contains("\"getPet\""), "{diagnostic:?}");
        assert!(parse_bytes(
            operation_id_document(&[("pets", "openapi"), ("stores", "openapi")], "getPet")
                .as_bytes()
        )
        .is_err());
    }

    #[test]
    fn bare_operation_id_counts_asyncapi_sources_as_non_arazzo() {
        let diagnostic =
            single_routing_error(&[("pets", "openapi"), ("events", "asyncapi")], "getPet");
        assert_eq!(diagnostic.kind, ValidationErrorKind::InvalidStepTarget);
    }

    #[test]
    fn bare_operation_id_excludes_arazzo_sources_from_the_count() {
        for sources in [
            &[("pets", "openapi"), ("flows", "arazzo")][..],
            &[("pets", "openapi")][..],
            &[("events", "asyncapi"), ("flows", "arazzo")][..],
        ] {
            assert!(
                operation_id_routing_diagnostics(sources, "getPet").is_empty(),
                "{sources:?}"
            );
            assert!(
                parse_bytes(operation_id_document(sources, "getPet").as_bytes()).is_ok(),
                "{sources:?}"
            );
        }
    }

    #[test]
    fn qualified_operation_id_naming_an_unknown_source_is_an_error() {
        let diagnostic = single_routing_error(
            &[("pets", "openapi"), ("stores", "openapi")],
            "$sourceDescriptions.nope.getPet",
        );
        assert_eq!(diagnostic.kind, ValidationErrorKind::InvalidReference);
        assert!(
            diagnostic
                .message
                .contains("references unknown sourceDescription \"nope\""),
            "{diagnostic:?}"
        );
    }

    #[test]
    fn qualified_operation_id_naming_an_arazzo_source_is_one_error() {
        let diagnostic = single_routing_error(
            &[("pets", "openapi"), ("flows", "arazzo")],
            "$sourceDescriptions.flows.getPet",
        );
        assert_eq!(diagnostic.kind, ValidationErrorKind::InvalidReference);
        assert!(
            diagnostic
                .message
                .contains("names sourceDescription \"flows\", which has type arazzo"),
            "{diagnostic:?}"
        );
        let document = operation_id_document(
            &[("pets", "openapi"), ("flows", "arazzo")],
            "$sourceDescriptions.flows.getPet",
        );
        match parse_bytes(document.as_bytes()) {
            Err(Error::Validation(report)) => assert_eq!(report.errors.len(), 1, "{report:?}"),
            other => panic!("expected one validation error, got: {other:?}"),
        }
    }

    #[test]
    fn qualified_operation_id_naming_an_asyncapi_source_is_conformant() {
        let sources = &[("pets", "openapi"), ("events", "asyncapi")];
        let operation_id = "$sourceDescriptions.events.placeOrder";
        assert!(operation_id_routing_diagnostics(sources, operation_id).is_empty());
        assert!(parse_bytes(operation_id_document(sources, operation_id).as_bytes()).is_ok());
    }

    #[test]
    fn malformed_qualified_operation_id_reports_the_classifier_reason() {
        for (operation_id, reason) in [
            (
                "$sourceDescriptions.pets",
                arazzo_spec::MalformedOperationId::MissingOperationId,
            ),
            (
                "$sourceDescriptions..getPet",
                arazzo_spec::MalformedOperationId::SourceName,
            ),
            (
                "$sourceDescriptions.pets.",
                arazzo_spec::MalformedOperationId::OperationId,
            ),
        ] {
            let diagnostic = single_routing_error(&[("pets", "openapi")], operation_id);
            assert_eq!(diagnostic.kind, ValidationErrorKind::InvalidStepTarget);
            assert!(
                diagnostic.message.contains(reason.as_str()),
                "{operation_id:?}: {diagnostic:?}"
            );
            assert!(
                diagnostic
                    .message
                    .contains(arazzo_spec::SUPPORTED_OPERATION_ID_FORMS),
                "{operation_id:?}: {diagnostic:?}"
            );
        }
    }

    #[test]
    fn qualified_operation_id_to_an_openapi_source_is_conformant() {
        for (sources, operation_id) in [
            (
                &[("pets", "openapi"), ("stores", "openapi")][..],
                "$sourceDescriptions.pets.getPet",
            ),
            (
                &[("a", "openapi"), ("stores", "openapi")][..],
                "$sourceDescriptions.a.b.c",
            ),
        ] {
            assert!(
                operation_id_routing_diagnostics(sources, operation_id).is_empty(),
                "{operation_id:?}"
            );
            assert!(
                parse_bytes(operation_id_document(sources, operation_id).as_bytes()).is_ok(),
                "{operation_id:?}"
            );
        }
    }
