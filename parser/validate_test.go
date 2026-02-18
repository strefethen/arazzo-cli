package parser

import (
	"strings"
	"testing"
)

// validSpec returns a minimal valid ArazzoSpec for use as a test baseline.
func validSpec() *ArazzoSpec {
	return &ArazzoSpec{
		Arazzo: "1.0.0",
		Info:   Info{Title: "Test", Version: "1.0.0"},
		SourceDescriptions: []SourceDescription{
			{Name: "api", URL: "https://example.com", Type: "openapi"},
		},
		Workflows: []Workflow{
			{
				WorkflowID: "wf1",
				Steps: []Step{
					{StepID: "s1", OperationPath: "/test"},
				},
			},
		},
	}
}

func TestValidate_ValidSpec(t *testing.T) {
	if err := Validate(validSpec()); err != nil {
		t.Fatalf("expected no error for valid spec, got: %v", err)
	}
}

// ── Arazzo version ──────────────────────────────────────────────────────

func TestValidate_MissingVersion(t *testing.T) {
	s := validSpec()
	s.Arazzo = ""
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for missing arazzo version")
	}
	if !strings.Contains(err.Error(), "arazzo version is required") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_UnsupportedVersion(t *testing.T) {
	s := validSpec()
	s.Arazzo = "2.0.0"
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for unsupported version")
	}
	if !strings.Contains(err.Error(), "unsupported arazzo version") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_Version1x(t *testing.T) {
	s := validSpec()
	s.Arazzo = "1.1.0"
	if err := Validate(s); err != nil {
		t.Fatalf("expected 1.1.0 to be accepted, got: %v", err)
	}
}

// ── Info ────────────────────────────────────────────────────────────────

func TestValidate_MissingTitle(t *testing.T) {
	s := validSpec()
	s.Info.Title = ""
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for missing title")
	}
	if !strings.Contains(err.Error(), "info.title is required") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_MissingInfoVersion(t *testing.T) {
	s := validSpec()
	s.Info.Version = ""
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for missing info version")
	}
	if !strings.Contains(err.Error(), "info.version is required") {
		t.Fatalf("unexpected error: %v", err)
	}
}

// ── Source descriptions ─────────────────────────────────────────────────

func TestValidate_SourceMissingName(t *testing.T) {
	s := validSpec()
	s.SourceDescriptions[0].Name = ""
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for missing source name")
	}
	if !strings.Contains(err.Error(), ".name is required") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_SourceDuplicateName(t *testing.T) {
	s := validSpec()
	s.SourceDescriptions = append(s.SourceDescriptions,
		SourceDescription{Name: "api", URL: "https://other.com", Type: "openapi"},
	)
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for duplicate source name")
	}
	if !strings.Contains(err.Error(), "is duplicate") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_SourceMissingURL(t *testing.T) {
	s := validSpec()
	s.SourceDescriptions[0].URL = ""
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for missing source URL")
	}
	if !strings.Contains(err.Error(), ".url is required") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_SourceInvalidType(t *testing.T) {
	s := validSpec()
	s.SourceDescriptions[0].Type = "graphql"
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for invalid source type")
	}
	if !strings.Contains(err.Error(), "must be 'openapi' or 'arazzo'") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_SourceTypeArazzo(t *testing.T) {
	s := validSpec()
	s.SourceDescriptions[0].Type = "arazzo"
	if err := Validate(s); err != nil {
		t.Fatalf("expected 'arazzo' type to be accepted, got: %v", err)
	}
}

// ── Workflows ───────────────────────────────────────────────────────────

func TestValidate_WorkflowMissingID(t *testing.T) {
	s := validSpec()
	s.Workflows[0].WorkflowID = ""
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for missing workflow ID")
	}
	if !strings.Contains(err.Error(), "workflowId is required") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_WorkflowDuplicateID(t *testing.T) {
	s := validSpec()
	s.Workflows = append(s.Workflows, Workflow{
		WorkflowID: "wf1",
		Steps:      []Step{{StepID: "s1", OperationPath: "/x"}},
	})
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for duplicate workflow ID")
	}
	if !strings.Contains(err.Error(), "is duplicate") {
		t.Fatalf("unexpected error: %v", err)
	}
}

// ── Steps ───────────────────────────────────────────────────────────────

func TestValidate_StepMissingID(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].StepID = ""
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for missing step ID")
	}
	if !strings.Contains(err.Error(), "stepId is required") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_StepDuplicateID(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps = append(s.Workflows[0].Steps,
		Step{StepID: "s1", OperationPath: "/other"},
	)
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for duplicate step ID")
	}
	if !strings.Contains(err.Error(), "is duplicate") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_StepNoOperation(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].OperationPath = ""
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for step with no operation")
	}
	if !strings.Contains(err.Error(), "must have operationId, operationPath, or workflowId") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_StepWithWorkflowID(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].OperationPath = ""
	s.Workflows[0].Steps[0].WorkflowID = "other-wf"
	if err := Validate(s); err != nil {
		t.Fatalf("expected step with workflowId to pass, got: %v", err)
	}
}

func TestValidate_StepWithOperationID(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].OperationPath = ""
	s.Workflows[0].Steps[0].OperationID = "getUser"
	if err := Validate(s); err != nil {
		t.Fatalf("expected step with operationId to pass, got: %v", err)
	}
}

// ── Parameters ──────────────────────────────────────────────────────────

func TestValidate_ParamMissingName(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].Parameters = []Parameter{
		{Value: "x", In: "query"},
	}
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for missing param name")
	}
	if !strings.Contains(err.Error(), ".name is required") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_ParamMissingValue(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].Parameters = []Parameter{
		{Name: "q", In: "query"},
	}
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for missing param value")
	}
	if !strings.Contains(err.Error(), "must have value or reference") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_ParamWithReference(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].Parameters = []Parameter{
		{Reference: "#/components/parameters/q"},
	}
	if err := Validate(s); err != nil {
		t.Fatalf("expected param with reference to pass, got: %v", err)
	}
}

func TestValidate_ParamInvalidIn(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].Parameters = []Parameter{
		{Name: "q", Value: "x", In: "body"},
	}
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for invalid param 'in'")
	}
	if !strings.Contains(err.Error(), "must be path, query, header, or cookie") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_ParamValidInValues(t *testing.T) {
	for _, in := range []string{"path", "query", "header", "cookie", ""} {
		s := validSpec()
		s.Workflows[0].Steps[0].Parameters = []Parameter{
			{Name: "q", Value: "x", In: in},
		}
		if err := Validate(s); err != nil {
			t.Fatalf("expected in=%q to pass, got: %v", in, err)
		}
	}
}

// ── Output references ───────────────────────────────────────────────────

func TestValidate_OutputReferencesUnknownStep(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Outputs = map[string]string{
		"result": "$steps.nonexistent.outputs.value",
	}
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for output referencing unknown step")
	}
	if !strings.Contains(err.Error(), "references unknown step 'nonexistent'") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestValidate_OutputReferencesValidStep(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Outputs = map[string]string{
		"result": "$steps.s1.outputs.value",
	}
	if err := Validate(s); err != nil {
		t.Fatalf("expected valid step reference to pass, got: %v", err)
	}
}

func TestValidate_OutputNonStepExpression(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Outputs = map[string]string{
		"result": "$inputs.name",
	}
	if err := Validate(s); err != nil {
		t.Fatalf("expected non-step output expression to pass, got: %v", err)
	}
}

// ── Multiple errors ─────────────────────────────────────────────────────

// ── Retry field validation ──────────────────────────────────────────────

func TestValidate_RetryAfterNegative(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].OnFailure = []OnAction{
		{Type: "retry", RetryAfter: -1},
	}
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for negative retryAfter")
	}
	if !strings.Contains(err.Error(), "retryAfter must be non-negative") {
		t.Fatalf("expected retryAfter error, got: %v", err)
	}
}

func TestValidate_RetryLimitNegative(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].OnFailure = []OnAction{
		{Type: "retry", RetryLimit: -5},
	}
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for negative retryLimit")
	}
	if !strings.Contains(err.Error(), "retryLimit must be non-negative") {
		t.Fatalf("expected retryLimit error, got: %v", err)
	}
}

func TestValidate_RetryFieldsValid(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].OnFailure = []OnAction{
		{Type: "retry", RetryAfter: 5, RetryLimit: 10},
	}
	if err := Validate(s); err != nil {
		t.Fatalf("expected no error for valid retry fields, got: %v", err)
	}
}

func TestValidate_RetryFieldsOnSuccess(t *testing.T) {
	s := validSpec()
	s.Workflows[0].Steps[0].OnSuccess = []OnAction{
		{Type: "retry", RetryAfter: -1},
	}
	err := Validate(s)
	if err == nil {
		t.Fatal("expected error for negative retryAfter on onSuccess")
	}
	if !strings.Contains(err.Error(), "retryAfter must be non-negative") {
		t.Fatalf("expected retryAfter error, got: %v", err)
	}
}

func TestValidate_MultipleErrors(t *testing.T) {
	s := &ArazzoSpec{} // everything missing
	err := Validate(s)
	if err == nil {
		t.Fatal("expected errors for empty spec")
	}
	// Should report at least version and title
	errStr := err.Error()
	if !strings.Contains(errStr, "arazzo version is required") {
		t.Fatalf("expected version error in: %v", errStr)
	}
	if !strings.Contains(errStr, "info.title is required") {
		t.Fatalf("expected title error in: %v", errStr)
	}
}
