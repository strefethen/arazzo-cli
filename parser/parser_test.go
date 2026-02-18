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
