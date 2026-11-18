use arazzo_expr::{classify_value_string, ValueStringSyntax};
use arazzo_spec::{media_type, RequestBody, ValueSource};

use crate::expressions::{validate_replacements, validate_value_source};
use crate::{check_unknown_fields, Diagnostic, ValidationErrorKind};

pub(crate) fn validate(
    step_path: &str,
    request_body: &RequestBody,
    arazzo_version: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let path = format!("{step_path}.requestBody");
    check_unknown_fields(&path, &request_body.extensions, diagnostics);
    if let Some(payload) = &request_body.payload {
        validate_value_source(
            &format!("{path}.payload"),
            payload,
            arazzo_version,
            diagnostics,
        );
    }
    validate_replacements(
        &format!("{path}.replacements"),
        &request_body.replacements,
        arazzo_version,
        diagnostics,
    );
    if let Some(diagnostic) = literal_json_text(&path, request_body) {
        diagnostics.push(diagnostic);
    }
}

fn literal_json_text(path: &str, request_body: &RequestBody) -> Option<Diagnostic> {
    let content_type = request_body.content_type.as_str();
    let declared = if content_type.is_empty() {
        "an omitted contentType, which sends application/json".to_owned()
    } else if media_type::is_json(content_type) {
        format!("contentType {content_type:?}")
    } else {
        return None;
    };
    let Some(ValueSource::Literal(serde_yaml_ng::Value::String(text))) = &request_body.payload
    else {
        return None;
    };
    if !matches!(
        classify_value_string(text),
        Ok(ValueStringSyntax::Literal(_))
    ) {
        return None;
    }
    let error = serde_json::from_str::<serde_json::Value>(text).err()?;
    let path = format!("{path}.payload");
    Some(Diagnostic::warning(
        ValidationErrorKind::InvalidRequestBody,
        path.clone(),
        format!(
            "{path} is a literal string sent as written under {declared}, but it is not JSON text: {error}"
        ),
    ))
}
