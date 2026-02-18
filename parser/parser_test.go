package parser

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const validYAML = `arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
`

// ── Parse ──────────────────────────────────────────────────────────────

func TestParse_FileNotFound(t *testing.T) {
	_, err := Parse("/nonexistent/path.yaml")
	if err == nil {
		t.Fatal("expected error for nonexistent file")
	}
	if !strings.Contains(err.Error(), "reading arazzo file") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestParse_ValidFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "spec.yaml")
	if err := os.WriteFile(path, []byte(validYAML), 0o644); err != nil {
		t.Fatal(err)
	}

	spec, err := Parse(path)
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if spec.Info.Title != "Test" {
		t.Fatalf("expected title 'Test', got %q", spec.Info.Title)
	}
}

// ── ParseBytes ─────────────────────────────────────────────────────────

func TestParseBytes_MalformedYAML(t *testing.T) {
	_, err := ParseBytes([]byte("[[["))
	if err == nil {
		t.Fatal("expected error for malformed YAML")
	}
	if !strings.Contains(err.Error(), "parsing arazzo yaml") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestParseBytes_ValidationError(t *testing.T) {
	// Valid YAML but missing required arazzo fields.
	_, err := ParseBytes([]byte("foo: bar\n"))
	if err == nil {
		t.Fatal("expected validation error")
	}
	if !strings.Contains(err.Error(), "arazzo version is required") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestParseBytes_ValidSpec(t *testing.T) {
	spec, err := ParseBytes([]byte(validYAML))
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if spec.Arazzo != "1.0.0" {
		t.Fatalf("expected arazzo '1.0.0', got %q", spec.Arazzo)
	}
	if len(spec.Workflows) != 1 {
		t.Fatalf("expected 1 workflow, got %d", len(spec.Workflows))
	}
	if spec.Workflows[0].Steps[0].StepID != "s1" {
		t.Fatalf("expected stepId 's1', got %q", spec.Workflows[0].Steps[0].StepID)
	}
}

// ── Components resolution ─────────────────────────────────────────────

func TestParseBytes_ComponentParameters(t *testing.T) {
	specYAML := `
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  parameters:
    authHeader:
      name: Authorization
      in: header
      value: "Bearer abc123"
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        parameters:
          - reference: "$components.parameters.authHeader"
`
	spec, err := ParseBytes([]byte(specYAML))
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}

	params := spec.Workflows[0].Steps[0].Parameters
	if len(params) != 1 {
		t.Fatalf("expected 1 parameter, got %d", len(params))
	}
	if params[0].Name != "Authorization" {
		t.Fatalf("expected name 'Authorization', got %q", params[0].Name)
	}
	if params[0].In != "header" {
		t.Fatalf("expected in 'header', got %q", params[0].In)
	}
	if params[0].Value != "Bearer abc123" {
		t.Fatalf("expected value 'Bearer abc123', got %q", params[0].Value)
	}
	if params[0].Reference != "" {
		t.Fatalf("expected reference to be cleared, got %q", params[0].Reference)
	}
}

func TestParseBytes_ComponentParameterOverride(t *testing.T) {
	specYAML := `
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  parameters:
    authHeader:
      name: Authorization
      in: header
      value: "Bearer default-token"
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        parameters:
          - reference: "$components.parameters.authHeader"
            value: "Bearer custom-token"
`
	spec, err := ParseBytes([]byte(specYAML))
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}

	// Step-level value should override component default
	params := spec.Workflows[0].Steps[0].Parameters
	if params[0].Value != "Bearer custom-token" {
		t.Fatalf("expected overridden value 'Bearer custom-token', got %q", params[0].Value)
	}
}

func TestParseBytes_ComponentParameterNotFound(t *testing.T) {
	specYAML := `
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  parameters: {}
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        parameters:
          - reference: "$components.parameters.missing"
`
	_, err := ParseBytes([]byte(specYAML))
	if err == nil {
		t.Fatal("expected error for missing component parameter")
	}
	if !strings.Contains(err.Error(), `component parameter "missing" not found`) {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestParseBytes_ComponentSuccessActions(t *testing.T) {
	specYAML := `
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  successActions:
    endWorkflow:
      - type: end
        name: terminate
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onSuccess:
          - name: "$components.successActions.endWorkflow"
`
	spec, err := ParseBytes([]byte(specYAML))
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}

	actions := spec.Workflows[0].Steps[0].OnSuccess
	if len(actions) != 1 {
		t.Fatalf("expected 1 success action, got %d", len(actions))
	}
	if actions[0].Type != "end" {
		t.Fatalf("expected action type 'end', got %q", actions[0].Type)
	}
	if actions[0].Name != "terminate" {
		t.Fatalf("expected action name 'terminate', got %q", actions[0].Name)
	}
}

func TestParseBytes_ComponentFailureActions(t *testing.T) {
	specYAML := `
arazzo: "1.0.0"
info:
  title: Test
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
components:
  failureActions:
    retryPolicy:
      - type: retry
        retryAfter: 2
        retryLimit: 5
workflows:
  - workflowId: wf1
    steps:
      - stepId: s1
        operationPath: /test
        onFailure:
          - name: "$components.failureActions.retryPolicy"
`
	spec, err := ParseBytes([]byte(specYAML))
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}

	actions := spec.Workflows[0].Steps[0].OnFailure
	if len(actions) != 1 {
		t.Fatalf("expected 1 failure action, got %d", len(actions))
	}
	if actions[0].Type != "retry" {
		t.Fatalf("expected action type 'retry', got %q", actions[0].Type)
	}
	if actions[0].RetryAfter != 2 {
		t.Fatalf("expected retryAfter=2, got %d", actions[0].RetryAfter)
	}
	if actions[0].RetryLimit != 5 {
		t.Fatalf("expected retryLimit=5, got %d", actions[0].RetryLimit)
	}
}

func TestParseBytes_NoComponents(t *testing.T) {
	// Spec without components should still work fine
	spec, err := ParseBytes([]byte(validYAML))
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if spec.Components != nil {
		t.Fatal("expected nil components for spec without components")
	}
}
