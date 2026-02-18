package runtime

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/strefethen/arazzo-cli/parser"
)

// newTestEngine creates an Engine with the given spec, pointed at a test server.
func newTestEngine(ts *httptest.Server, spec *parser.ArazzoSpec) *Engine {
	// Override the sourceDescription URL to point at the test server
	if len(spec.SourceDescriptions) > 0 {
		spec.SourceDescriptions[0].URL = ts.URL
	}
	return NewEngine(spec)
}

// makeSpec is a helper to build a minimal ArazzoSpec with one workflow.
func makeSpec(workflows ...parser.Workflow) *parser.ArazzoSpec {
	return &parser.ArazzoSpec{
		Arazzo: "1.0.0",
		Info:   parser.Info{Title: "test", Version: "1.0.0"},
		SourceDescriptions: []parser.SourceDescription{
			{Name: "test", URL: "http://localhost", Type: "openapi"},
		},
		Workflows: workflows,
	}
}

func TestExecute_SequentialSteps(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case "/step1":
			w.WriteHeader(200)
			_, _ = io.WriteString(w,`{"value":"hello"}`)
		case "/step2":
			w.WriteHeader(200)
			_, _ = io.WriteString(w,`{"result":"world"}`)
		default:
			w.WriteHeader(404)
		}
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "sequential",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/step1",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
			},
			{
				StepID:        "s2",
				OperationPath: "/step2",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	outputs, err := engine.Execute(context.Background(), "sequential", nil)
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if outputs == nil {
		t.Fatal("expected non-nil outputs")
	}
}

func TestExecute_FailureNoHandler(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
		_, _ = io.WriteString(w,`{"error":"server error"}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "fail-no-handler",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/fail",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "fail-no-handler", nil)
	if err == nil {
		t.Fatal("expected error for unhandled failure")
	}
	expectedPrefix := "step s1: success criteria not met (status=500"
	if !strings.Contains(err.Error(), expectedPrefix) {
		t.Fatalf("expected error containing %q, got %q", expectedPrefix, err.Error())
	}
}

func TestExecute_OnFailureEnd(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "fail-end",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/fail",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{Type: "end"},
				},
			},
			{
				StepID:        "s2",
				OperationPath: "/should-not-reach",
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "fail-end", nil)
	if err == nil {
		t.Fatal("expected error from onFailure end action")
	}
	expected := "step s1: workflow ended by onFailure action"
	if err.Error() != expected {
		t.Fatalf("expected error %q, got %q", expected, err.Error())
	}
}

func TestExecute_OnSuccessEnd(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(200)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "success-end",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/ok",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnSuccess: []parser.OnAction{
					{Type: "end"},
				},
			},
			{
				StepID:        "s2",
				OperationPath: "/should-not-reach",
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "success-end", nil)
	if err != nil {
		t.Fatalf("expected no error from onSuccess end, got: %v", err)
	}
}

func TestExecute_OnFailureGoto(t *testing.T) {
	var paths []string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		paths = append(paths, r.URL.Path)
		switch r.URL.Path {
		case "/fail":
			w.WriteHeader(500)
		case "/fallback":
			w.WriteHeader(200)
			_, _ = io.WriteString(w,`{"fallback":true}`)
		default:
			w.WriteHeader(404)
		}
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "fail-goto",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/fail",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{Type: "goto", StepID: "fallback"},
				},
			},
			{
				StepID:        "skipped",
				OperationPath: "/should-not-reach",
			},
			{
				StepID:        "fallback",
				OperationPath: "/fallback",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "fail-goto", nil)
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}

	// Verify the skipped step was not called
	for _, p := range paths {
		if p == "/should-not-reach" {
			t.Fatal("skipped step should not have been reached")
		}
	}
	if len(paths) != 2 {
		t.Fatalf("expected 2 requests (/fail, /fallback), got %d: %v", len(paths), paths)
	}
}

func TestExecute_OnSuccessGoto(t *testing.T) {
	var paths []string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		paths = append(paths, r.URL.Path)
		w.WriteHeader(200)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "success-goto",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/start",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnSuccess: []parser.OnAction{
					{Type: "goto", StepID: "s3"},
				},
			},
			{
				StepID:        "s2",
				OperationPath: "/skipped",
			},
			{
				StepID:        "s3",
				OperationPath: "/target",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "success-goto", nil)
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}

	for _, p := range paths {
		if p == "/skipped" {
			t.Fatal("skipped step should not have been reached")
		}
	}
}

func TestExecute_OnFailureRetry(t *testing.T) {
	callCount := 0
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		callCount++
		if callCount < 3 {
			w.WriteHeader(500)
			return
		}
		w.WriteHeader(200)
		_, _ = io.WriteString(w,`{"ok":true}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "retry",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/flaky",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{Type: "retry"},
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "retry", nil)
	if err != nil {
		t.Fatalf("expected success after retries, got: %v", err)
	}
	if callCount != 3 {
		t.Fatalf("expected 3 calls (2 failures + 1 success), got %d", callCount)
	}
}

func TestExecute_RetryExceedsMax(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "retry-max",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/always-fail",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{Type: "retry"},
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "retry-max", nil)
	if err == nil {
		t.Fatal("expected error after max retries exceeded")
	}
	expected := "step s1: max retries (3) exceeded"
	if err.Error() != expected {
		t.Fatalf("expected error %q, got %q", expected, err.Error())
	}
}

// ── Retry policy tests ─────────────────────────────────────────────────

func TestExecute_RetryCustomLimit(t *testing.T) {
	callCount := 0
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		callCount++
		if callCount <= 5 {
			w.WriteHeader(500)
			return
		}
		w.WriteHeader(200)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "retry-limit",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/flaky",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{Type: "retry", RetryLimit: 6},
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "retry-limit", nil)
	if err != nil {
		t.Fatalf("expected success with retryLimit=6, got: %v", err)
	}
	if callCount != 6 {
		t.Fatalf("expected 6 calls (5 failures + 1 success), got %d", callCount)
	}
}

func TestExecute_RetryCustomLimitExceeded(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "retry-limit-exceeded",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/always-fail",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{Type: "retry", RetryLimit: 2},
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "retry-limit-exceeded", nil)
	if err == nil {
		t.Fatal("expected error after custom retry limit exceeded")
	}
	expected := "step s1: max retries (2) exceeded"
	if err.Error() != expected {
		t.Fatalf("expected error %q, got %q", expected, err.Error())
	}
}

func TestExecute_RetryWithDelay(t *testing.T) {
	callCount := 0
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		callCount++
		if callCount < 2 {
			w.WriteHeader(500)
			return
		}
		w.WriteHeader(200)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "retry-delay",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/flaky",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{Type: "retry", RetryAfter: 1, RetryLimit: 3},
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	start := time.Now()
	_, err := engine.Execute(context.Background(), "retry-delay", nil)
	elapsed := time.Since(start)

	if err != nil {
		t.Fatalf("expected success, got: %v", err)
	}
	// Should have waited ~1 second for the retry delay
	if elapsed < 900*time.Millisecond {
		t.Fatalf("expected >= 900ms delay from retryAfter, got %v", elapsed)
	}
}

func TestExecute_RetryDelayCancelledByContext(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "retry-cancel",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/fail",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{Type: "retry", RetryAfter: 60, RetryLimit: 5}, // 60s delay
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
	defer cancel()

	start := time.Now()
	_, err := engine.Execute(ctx, "retry-cancel", nil)
	elapsed := time.Since(start)

	if err == nil {
		t.Fatal("expected context cancellation error")
	}
	if elapsed > 2*time.Second {
		t.Fatalf("expected fast cancellation, took %v", elapsed)
	}
}

func TestExecute_OnFailureCriteriaMatching(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/main":
			w.WriteHeader(429)
		case "/rate-limit-handler":
			w.WriteHeader(200)
		default:
			w.WriteHeader(404)
		}
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "criteria-match",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/main",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					// First action: only matches 429
					{
						Type:   "goto",
						StepID: "rate-handler",
						Criteria: []parser.SuccessCriterion{
							{Condition: "$statusCode == 429"},
						},
					},
					// Second action: matches 500
					{
						Type:   "goto",
						StepID: "server-error-handler",
						Criteria: []parser.SuccessCriterion{
							{Condition: "$statusCode == 500"},
						},
					},
					// Default: end
					{Type: "end"},
				},
			},
			{
				StepID:        "rate-handler",
				OperationPath: "/rate-limit-handler",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
			},
			{
				StepID:        "server-error-handler",
				OperationPath: "/should-not-reach",
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "criteria-match", nil)
	if err != nil {
		t.Fatalf("expected no error (should goto rate-handler), got: %v", err)
	}
}

func TestExecute_OnFailureCriteriaNoneMatch(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(418) // I'm a teapot - won't match any criteria
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "no-criteria-match",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/teapot",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{
						Type: "retry",
						Criteria: []parser.SuccessCriterion{
							{Condition: "$statusCode == 429"},
						},
					},
					{
						Type:   "goto",
						StepID: "handler",
						Criteria: []parser.SuccessCriterion{
							{Condition: "$statusCode == 500"},
						},
					},
					// No default fallback — should fail as if no onFailure
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "no-criteria-match", nil)
	if err == nil {
		t.Fatal("expected error when no criteria match")
	}
	expectedPrefix := "step s1: success criteria not met (status=418"
	if !strings.Contains(err.Error(), expectedPrefix) {
		t.Fatalf("expected error containing %q, got %q", expectedPrefix, err.Error())
	}
}

func TestExecute_GotoNonExistentStep(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "bad-goto",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/fail",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{Type: "goto", StepID: "nonexistent"},
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "bad-goto", nil)
	if err == nil {
		t.Fatal("expected error for goto to nonexistent step")
	}
	expected := `goto: step "nonexistent" not found`
	if err.Error() != expected {
		t.Fatalf("expected error %q, got %q", expected, err.Error())
	}
}

func TestExecute_GotoNoStepID(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "goto-no-stepid",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/fail",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{Type: "goto"}, // no stepId
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "goto-no-stepid", nil)
	if err == nil {
		t.Fatal("expected error for goto without stepId")
	}
	expected := "goto: no stepId or workflowId specified"
	if err.Error() != expected {
		t.Fatalf("expected error %q, got %q", expected, err.Error())
	}
}

func TestExecute_WorkflowNotFound(t *testing.T) {
	spec := makeSpec()
	engine := NewEngine(spec)
	_, err := engine.Execute(context.Background(), "nonexistent", nil)
	if err == nil {
		t.Fatal("expected error for nonexistent workflow")
	}
	expected := `workflow "nonexistent" not found`
	if err.Error() != expected {
		t.Fatalf("expected error %q, got %q", expected, err.Error())
	}
}

func TestExecute_DefaultBehaviorNoOnSuccess(t *testing.T) {
	// Without onSuccess, steps proceed sequentially
	var paths []string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		paths = append(paths, r.URL.Path)
		w.WriteHeader(200)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "default-seq",
		Steps: []parser.Step{
			{StepID: "s1", OperationPath: "/a", SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}}},
			{StepID: "s2", OperationPath: "/b", SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}}},
			{StepID: "s3", OperationPath: "/c", SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}}},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "default-seq", nil)
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if len(paths) != 3 || paths[0] != "/a" || paths[1] != "/b" || paths[2] != "/c" {
		t.Fatalf("expected sequential execution [/a /b /c], got %v", paths)
	}
}

func TestExecute_RetryThenGoto(t *testing.T) {
	callCount := 0
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		callCount++
		switch r.URL.Path {
		case "/flaky":
			if callCount <= 2 {
				w.WriteHeader(429)
			} else {
				w.WriteHeader(200)
			}
		default:
			w.WriteHeader(200)
		}
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "retry-then-success",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/flaky",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnFailure: []parser.OnAction{
					{
						Type: "retry",
						Criteria: []parser.SuccessCriterion{
							{Condition: "$statusCode == 429"},
						},
					},
					{Type: "end"}, // other failures = end
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "retry-then-success", nil)
	if err != nil {
		t.Fatalf("expected success after retries, got: %v", err)
	}
	if callCount != 3 {
		t.Fatalf("expected 3 calls, got %d", callCount)
	}
}

func TestFindMatchingAction_NilResponse(t *testing.T) {
	engine := &Engine{}
	vars := NewVarStore()

	// Action with no criteria should match even with nil response
	actions := []parser.OnAction{
		{Type: "end"},
	}
	action := engine.findMatchingAction(actions, vars, nil)
	if action == nil {
		t.Fatal("expected match for action with no criteria")
	}
	if action.Type != "end" {
		t.Fatalf("expected type 'end', got %q", action.Type)
	}
}

func TestFindMatchingAction_NilResponseWithCriteria(t *testing.T) {
	engine := &Engine{}
	vars := NewVarStore()

	// Action with criteria that need response should not match when response is nil
	actions := []parser.OnAction{
		{
			Type: "retry",
			Criteria: []parser.SuccessCriterion{
				{Condition: "$statusCode == 429"},
			},
		},
	}
	action := engine.findMatchingAction(actions, vars, nil)
	if action != nil {
		t.Fatal("expected no match when response is nil and criteria need statusCode")
	}
}

func TestFindMatchingAction_FirstMatchWins(t *testing.T) {
	engine := &Engine{}
	vars := NewVarStore()
	resp := &Response{StatusCode: 429}

	actions := []parser.OnAction{
		{
			Name: "first",
			Type: "retry",
			Criteria: []parser.SuccessCriterion{
				{Condition: "$statusCode == 429"},
			},
		},
		{
			Name: "second",
			Type: "end",
		},
	}
	action := engine.findMatchingAction(actions, vars, resp)
	if action == nil {
		t.Fatal("expected a match")
	}
	if action.Name != "first" {
		t.Fatalf("expected first action to win, got %q", action.Name)
	}
}

func TestFindStepIndex(t *testing.T) {
	wf := &parser.Workflow{
		WorkflowID: "test-wf",
		Steps: []parser.Step{
			{StepID: "a"},
			{StepID: "b"},
			{StepID: "c"},
		},
	}
	spec := makeSpec(*wf)
	engine := NewEngine(spec)

	if idx := engine.findStepIndex(wf, "b"); idx != 1 {
		t.Fatalf("expected index 1 for step 'b', got %d", idx)
	}
	if idx := engine.findStepIndex(wf, "nonexistent"); idx != -1 {
		t.Fatalf("expected -1 for nonexistent step, got %d", idx)
	}
}

func TestBuildOutputs(t *testing.T) {
	engine := &Engine{}
	vars := NewVarStore()
	vars.SetInput("name", "test")
	vars.SetStepOutput("s1", "result", "hello")

	wf := &parser.Workflow{
		Outputs: map[string]string{
			"inputName":  "$inputs.name",
			"stepResult": "$steps.s1.outputs.result",
		},
	}

	outputs := engine.buildOutputs(wf, vars)
	if outputs["inputName"] != "test" {
		t.Fatalf("expected inputName='test', got %v", outputs["inputName"])
	}
	if outputs["stepResult"] != "hello" {
		t.Fatalf("expected stepResult='hello', got %v", outputs["stepResult"])
	}
}

func TestExecute_UnknownActionType(t *testing.T) {
	// Unknown action type defaults to moving to next step
	var paths []string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		paths = append(paths, r.URL.Path)
		w.WriteHeader(200)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "unknown-action",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/a",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				OnSuccess: []parser.OnAction{
					{Type: "unknown-type"},
				},
			},
			{
				StepID:        "s2",
				OperationPath: "/b",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
			},
		},
	})

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "unknown-action", nil)
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if len(paths) != 2 {
		t.Fatalf("expected 2 paths, got %d: %v", len(paths), paths)
	}
}

func TestExecute_ResponseHeaderExpression(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Request-Id", "abc-123")
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"ok":true}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "header-extract",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/test",
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				Outputs: map[string]string{
					"request_id": "$response.header.X-Request-Id",
				},
			},
		},
		Outputs: map[string]string{
			"request_id": "$steps.s1.outputs.request_id",
		},
	})

	engine := newTestEngine(ts, spec)
	outputs, err := engine.Execute(context.Background(), "header-extract", nil)
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if outputs["request_id"] != "abc-123" {
		t.Fatalf("expected request_id='abc-123', got %v", outputs["request_id"])
	}
}

func TestExecute_EnvExpression(t *testing.T) {
	t.Setenv("ARAZZO_TEST_TOKEN", "secret-42")

	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"auth":"`+r.Header.Get("Authorization")+`"}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "env-test",
		Steps: []parser.Step{
			{
				StepID:        "s1",
				OperationPath: "/protected",
				Parameters: []parser.Parameter{
					{Name: "Authorization", In: "header", Value: "$env.ARAZZO_TEST_TOKEN"},
				},
				SuccessCriteria: []parser.SuccessCriterion{
					{Condition: "$statusCode == 200"},
				},
				Outputs: map[string]string{
					"auth": "$response.body.auth",
				},
			},
		},
		Outputs: map[string]string{
			"auth": "$steps.s1.outputs.auth",
		},
	})

	engine := newTestEngine(ts, spec)
	outputs, err := engine.Execute(context.Background(), "env-test", nil)
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if outputs["auth"] != "secret-42" {
		t.Fatalf("expected auth='secret-42', got %v", outputs["auth"])
	}
}

func TestExecute_EnvExpressionUnset(t *testing.T) {
	t.Setenv("ARAZZO_TEST_MISSING", "") // set empty, Setenv restores on cleanup

	vars := NewVarStore()
	eval := NewExpressionEvaluator(vars)
	val := eval.Evaluate("$env.ARAZZO_TEST_MISSING")
	if val != "" {
		t.Fatalf("expected empty string for unset env var, got %v", val)
	}
}

func TestBuildURL_QueryParamsEncoded(t *testing.T) {
	engine := &Engine{baseURL: "http://localhost"}
	vars := NewVarStore()
	vars.SetInput("q", "hello world&more=stuff")

	step := parser.Step{
		OperationPath: "/search",
		Parameters: []parser.Parameter{
			{Name: "q", In: "query", Value: "$inputs.q"},
			{Name: "tag", In: "query", Value: "a=b"},
		},
	}

	got := engine.buildURL(step, vars)

	// Verify the URL parses cleanly
	u, err := url.Parse(got)
	if err != nil {
		t.Fatalf("buildURL produced unparseable URL: %v", err)
	}

	if q := u.Query().Get("q"); q != "hello world&more=stuff" {
		t.Fatalf("expected q='hello world&more=stuff', got %q", q)
	}
	if tag := u.Query().Get("tag"); tag != "a=b" {
		t.Fatalf("expected tag='a=b', got %q", tag)
	}
}

// Benchmarks for regex optimization verification

func BenchmarkBuildURI(b *testing.B) {
	engine := &Engine{
		baseURL: "http://localhost:8080",
	}
	vars := NewVarStore()
	vars.SetInput("userId", "12345")
	vars.SetInput("type", "premium")

	step := parser.Step{
		OperationPath: "/api/users/{userId}/subscriptions/{type}",
		Parameters: []parser.Parameter{
			{Name: "userId", In: "path", Value: "$inputs.userId"},
			{Name: "type", In: "path", Value: "$inputs.type"},
			{Name: "includeDetails", In: "query", Value: "true"},
			{Name: "format", In: "query", Value: "json"},
		},
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = engine.buildURL(step, vars)
	}
}

func BenchmarkToGJSONPath(b *testing.B) {
	testCases := []string{
		"$response.body.data.items[0].name",
		"$response.body.users[5].profile.address[2]",
		"$response.body[0].value",
	}

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		for _, tc := range testCases {
			_ = toGJSONPath(tc)
		}
	}
}

func BenchmarkInterpolateString(b *testing.B) {
	vars := NewVarStore()
	vars.SetInput("name", "Alice")
	vars.SetInput("age", 30)
	vars.SetStepOutput("s1", "result", "success")

	eval := NewExpressionEvaluator(vars)
	testString := "User ${inputs.name} is $inputs.age years old. Status: ${steps.s1.outputs.result}"

	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		_ = eval.InterpolateString(testString)
	}
}

func BenchmarkParseMethod(b *testing.B) {
	inputs := []string{
		"PUT /users/{id}",
		"DELETE /items/{id}",
		"GET /health",
		"/no-method/path",
		"OPTIONS /cors",
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		for _, in := range inputs {
			_, _ = parseMethod(in)
		}
	}
}

func BenchmarkEvaluateCondition(b *testing.B) {
	vars := NewVarStore()
	eval := NewExpressionEvaluator(vars)
	eval.WithResponse(&Response{StatusCode: 200, Body: []byte(`{}`)})

	conditions := []string{
		"$statusCode == 200",
		"$statusCode != 404",
		"$statusCode == 201",
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		for _, c := range conditions {
			_ = eval.EvaluateCondition(c)
		}
	}
}

func BenchmarkEvaluateCriterionRegex(b *testing.B) {
	vars := NewVarStore()
	eval := NewExpressionEvaluator(vars)
	eval.WithResponse(&Response{StatusCode: 200, Body: []byte(`{}`)})

	criterion := parser.SuccessCriterion{
		Type:      "regex",
		Context:   "$statusCode",
		Condition: `^2\d{2}$`,
	}
	resp := &Response{StatusCode: 200, Body: []byte(`{}`)}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		evaluateCriterion(criterion, eval, resp)
	}
}

func BenchmarkEvaluate(b *testing.B) {
	vars := NewVarStore()
	vars.SetInput("name", "Alice")
	vars.SetStepOutput("s1", "result", "success")
	eval := NewExpressionEvaluator(vars)

	exprs := []string{
		"$inputs.name",
		"$steps.s1.outputs.result",
		"literal-value",
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		for _, expr := range exprs {
			_ = eval.Evaluate(expr)
		}
	}
}

func BenchmarkToGJSONPath_NoBrackets(b *testing.B) {
	// Fast path: no array indexing needed
	inputs := []string{
		"$response.body.data.name",
		"$response.body.users.profile.address",
		"$response.body.simple",
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		for _, in := range inputs {
			_ = toGJSONPath(in)
		}
	}
}

// ── Bug regression tests ───────────────────────────────────────────────

func TestBuildURL_NoDoubleSlash(t *testing.T) {
	// Regression: baseURL with trailing slash + operationPath with leading
	// slash previously produced "//".
	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps:      []parser.Step{{StepID: "s1", OperationPath: "/users"}},
	})
	spec.SourceDescriptions[0].URL = "https://api.example.com/"
	e := NewEngine(spec)
	vars := NewVarStore()

	got := e.buildURL(spec.Workflows[0].Steps[0], vars)
	if strings.Contains(got, "//users") {
		t.Fatalf("double slash in URL: %s", got)
	}
	if got != "https://api.example.com/users" {
		t.Fatalf("expected https://api.example.com/users, got %s", got)
	}
}

func TestExecute_RequestBodyContentType(t *testing.T) {
	// Regression: RequestBody.ContentType was ignored, always sent application/json.
	var gotContentType string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotContentType = r.Header.Get("Content-Type")
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"ok":true}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{{
			StepID:        "s1",
			OperationPath: "/submit",
			RequestBody: &parser.RequestBody{
				ContentType: "application/x-www-form-urlencoded",
				Payload:     map[string]any{"key": "val"},
			},
			SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}},
		}},
	})
	e := newTestEngine(ts, spec)
	_, err := e.Execute(context.Background(), "wf", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if gotContentType != "application/x-www-form-urlencoded" {
		t.Fatalf("expected Content-Type 'application/x-www-form-urlencoded', got %q", gotContentType)
	}
}

func TestExecute_RequestBodyDefaultContentType(t *testing.T) {
	// When ContentType is empty, should default to application/json.
	var gotContentType string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotContentType = r.Header.Get("Content-Type")
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"ok":true}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{{
			StepID:        "s1",
			OperationPath: "/submit",
			RequestBody: &parser.RequestBody{
				Payload: map[string]any{"key": "val"},
			},
			SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}},
		}},
	})
	e := newTestEngine(ts, spec)
	_, err := e.Execute(context.Background(), "wf", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if gotContentType != "application/json" {
		t.Fatalf("expected default Content-Type 'application/json', got %q", gotContentType)
	}
}

// ── HTTP method support ────────────────────────────────────────────────

func TestParseMethod(t *testing.T) {
	tests := []struct {
		input      string
		wantMethod string
		wantPath   string
	}{
		{"PUT /users/{id}", "PUT", "/users/{id}"},
		{"DELETE /users/{id}", "DELETE", "/users/{id}"},
		{"PATCH /items/1", "PATCH", "/items/1"},
		{"GET /health", "GET", "/health"},
		{"HEAD /ping", "HEAD", "/ping"},
		{"OPTIONS /api", "OPTIONS", "/api"},
		{"/users", "", "/users"},
		{"", "", ""},
		{"POST /data", "POST", "/data"},
	}
	for _, tt := range tests {
		method, path := parseMethod(tt.input)
		if method != tt.wantMethod || path != tt.wantPath {
			t.Errorf("parseMethod(%q) = (%q, %q), want (%q, %q)",
				tt.input, method, path, tt.wantMethod, tt.wantPath)
		}
	}
}

func TestExecute_PutMethod(t *testing.T) {
	var gotMethod string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotMethod = r.Method
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"updated":true}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{{
			StepID:        "s1",
			OperationPath: "PUT /users/123",
			RequestBody:   &parser.RequestBody{Payload: map[string]any{"name": "Alice"}},
			SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}},
		}},
	})
	e := newTestEngine(ts, spec)
	_, err := e.Execute(context.Background(), "wf", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if gotMethod != "PUT" {
		t.Fatalf("expected PUT, got %s", gotMethod)
	}
}

func TestExecute_DeleteMethod(t *testing.T) {
	var gotMethod string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotMethod = r.Method
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(204)
		_, _ = io.WriteString(w, `{}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{{
			StepID:          "s1",
			OperationPath:   "DELETE /users/123",
			SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 204"}},
		}},
	})
	e := newTestEngine(ts, spec)
	_, err := e.Execute(context.Background(), "wf", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if gotMethod != "DELETE" {
		t.Fatalf("expected DELETE, got %s", gotMethod)
	}
}

func TestExecute_PatchMethod(t *testing.T) {
	var gotMethod string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotMethod = r.Method
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"patched":true}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{{
			StepID:        "s1",
			OperationPath: "PATCH /items/42",
			RequestBody:   &parser.RequestBody{Payload: map[string]any{"status": "active"}},
			SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}},
		}},
	})
	e := newTestEngine(ts, spec)
	_, err := e.Execute(context.Background(), "wf", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if gotMethod != "PATCH" {
		t.Fatalf("expected PATCH, got %s", gotMethod)
	}
}

// ── Criterion type tests ───────────────────────────────────────────────

func TestEvaluateCriterion_Simple(t *testing.T) {
	resp := &Response{StatusCode: 200}
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(resp)
	c := parser.SuccessCriterion{Condition: "$statusCode == 200"}
	if !evaluateCriterion(c, eval, resp) {
		t.Fatal("expected simple criterion to pass")
	}
	c.Condition = "$statusCode == 404"
	if evaluateCriterion(c, eval, resp) {
		t.Fatal("expected simple criterion to fail")
	}
}

func TestEvaluateCriterion_SimpleExplicitType(t *testing.T) {
	resp := &Response{StatusCode: 200}
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(resp)
	c := parser.SuccessCriterion{Type: "simple", Condition: "$statusCode == 200"}
	if !evaluateCriterion(c, eval, resp) {
		t.Fatal("expected explicit simple criterion to pass")
	}
}

func TestEvaluateCriterion_Regex(t *testing.T) {
	resp := &Response{StatusCode: 200}
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(resp)
	c := parser.SuccessCriterion{
		Type:      "regex",
		Context:   "$statusCode",
		Condition: "^2\\d{2}$", // matches 2xx
	}
	if !evaluateCriterion(c, eval, resp) {
		t.Fatal("expected regex criterion to match 200")
	}
}

func TestEvaluateCriterion_RegexNoMatch(t *testing.T) {
	resp := &Response{StatusCode: 500}
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(resp)
	c := parser.SuccessCriterion{
		Type:      "regex",
		Context:   "$statusCode",
		Condition: "^2\\d{2}$",
	}
	if evaluateCriterion(c, eval, resp) {
		t.Fatal("expected regex criterion to not match 500")
	}
}

func TestEvaluateCriterion_RegexBodyContent(t *testing.T) {
	vars := NewVarStore()
	vars.SetStepOutput("s1", "name", "Hello World 123")
	eval := NewExpressionEvaluator(vars)
	c := parser.SuccessCriterion{
		Type:      "regex",
		Context:   "$steps.s1.outputs.name",
		Condition: `^Hello\s+World\s+\d+$`,
	}
	if !evaluateCriterion(c, eval, nil) {
		t.Fatal("expected regex to match step output")
	}
}

func TestEvaluateCriterion_JSONPath(t *testing.T) {
	resp := &Response{
		Body:        []byte(`{"users":[{"name":"Alice"},{"name":"Bob"}]}`),
		ContentType: "json",
	}
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(resp)
	c := parser.SuccessCriterion{
		Type:      "jsonpath",
		Condition: "users.0.name", // gjson path
	}
	if !evaluateCriterion(c, eval, resp) {
		t.Fatal("expected jsonpath to find users.0.name")
	}
}

func TestEvaluateCriterion_JSONPathMissing(t *testing.T) {
	resp := &Response{
		Body:        []byte(`{"users":[]}`),
		ContentType: "json",
	}
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(resp)
	c := parser.SuccessCriterion{
		Type:      "jsonpath",
		Condition: "nonexistent.path",
	}
	if evaluateCriterion(c, eval, resp) {
		t.Fatal("expected jsonpath to fail for missing path")
	}
}

func TestEvaluateCriterion_JSONPathNoResponse(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	c := parser.SuccessCriterion{
		Type:      "jsonpath",
		Condition: "users.0.name",
	}
	if evaluateCriterion(c, eval, nil) {
		t.Fatal("expected jsonpath to fail with nil response")
	}
}

func TestExecute_RegexCriterion(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(201)
		_, _ = io.WriteString(w, `{"id":"abc-123"}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "regex-criterion",
		Steps: []parser.Step{{
			StepID:        "s1",
			OperationPath: "/create",
			SuccessCriteria: []parser.SuccessCriterion{
				{
					Type:      "regex",
					Context:   "$statusCode",
					Condition: "^2\\d{2}$", // any 2xx
				},
			},
		}},
	})
	e := newTestEngine(ts, spec)
	_, err := e.Execute(context.Background(), "regex-criterion", nil)
	if err != nil {
		t.Fatalf("expected regex criterion to pass for 201, got: %v", err)
	}
}

func TestExecute_RegexCriterionFails(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
		_, _ = io.WriteString(w, `{"error":"fail"}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "regex-fail",
		Steps: []parser.Step{{
			StepID:        "s1",
			OperationPath: "/fail",
			SuccessCriteria: []parser.SuccessCriterion{
				{
					Type:      "regex",
					Context:   "$statusCode",
					Condition: "^2\\d{2}$",
				},
			},
		}},
	})
	e := newTestEngine(ts, spec)
	_, err := e.Execute(context.Background(), "regex-fail", nil)
	if err == nil {
		t.Fatal("expected error when regex criterion fails")
	}
}

func TestExecute_JSONPathCriterion(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"data":{"items":[{"id":1},{"id":2}]}}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "jsonpath-criterion",
		Steps: []parser.Step{{
			StepID:        "s1",
			OperationPath: "/data",
			SuccessCriteria: []parser.SuccessCriterion{
				{Condition: "$statusCode == 200"},
				{
					Type:      "jsonpath",
					Condition: "data.items.0.id", // gjson: check first item exists
				},
			},
		}},
	})
	e := newTestEngine(ts, spec)
	_, err := e.Execute(context.Background(), "jsonpath-criterion", nil)
	if err != nil {
		t.Fatalf("expected jsonpath criterion to pass, got: %v", err)
	}
}

func TestExecute_JSONPathCriterionFails(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"data":{"items":[]}}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "jsonpath-fail",
		Steps: []parser.Step{{
			StepID:        "s1",
			OperationPath: "/data",
			SuccessCriteria: []parser.SuccessCriterion{
				{Condition: "$statusCode == 200"},
				{
					Type:      "jsonpath",
					Condition: "data.items.0.id", // empty array → no match
				},
			},
		}},
	})
	e := newTestEngine(ts, spec)
	_, err := e.Execute(context.Background(), "jsonpath-fail", nil)
	if err == nil {
		t.Fatal("expected error when jsonpath criterion fails on empty array")
	}
}

func TestExecute_MethodFallbackGET(t *testing.T) {
	var gotMethod string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotMethod = r.Method
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"ok":true}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{{
			StepID:          "s1",
			OperationPath:   "/health",
			SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}},
		}},
	})
	e := newTestEngine(ts, spec)
	_, err := e.Execute(context.Background(), "wf", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if gotMethod != "GET" {
		t.Fatalf("expected fallback GET, got %s", gotMethod)
	}
}

// ── Sub-workflow tests ─────────────────────────────────────────────────

func TestExecute_SubWorkflowStep(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"token":"xyz-789"}`)
	}))
	defer ts.Close()

	spec := makeSpec(
		// Parent workflow
		parser.Workflow{
			WorkflowID: "parent",
			Steps: []parser.Step{
				{
					StepID:     "call-child",
					WorkflowID: "child",
					Outputs: map[string]string{
						"childToken": "$steps.call-child.outputs.token",
					},
				},
			},
			Outputs: map[string]string{
				"token": "$steps.call-child.outputs.token",
			},
		},
		// Child workflow
		parser.Workflow{
			WorkflowID: "child",
			Steps: []parser.Step{
				{
					StepID:        "get-token",
					OperationPath: "/auth",
					SuccessCriteria: []parser.SuccessCriterion{
						{Condition: "$statusCode == 200"},
					},
					Outputs: map[string]string{
						"token": "$response.body.token",
					},
				},
			},
			Outputs: map[string]string{
				"token": "$steps.get-token.outputs.token",
			},
		},
	)

	engine := newTestEngine(ts, spec)
	outputs, err := engine.Execute(context.Background(), "parent", nil)
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if outputs["token"] != "xyz-789" {
		t.Fatalf("expected token='xyz-789', got %v", outputs["token"])
	}
}

func TestExecute_SubWorkflowWithInputs(t *testing.T) {
	var gotPath string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotPath = r.URL.Path
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"name":"Alice"}`)
	}))
	defer ts.Close()

	spec := makeSpec(
		parser.Workflow{
			WorkflowID: "parent",
			Steps: []parser.Step{
				{
					StepID:     "call-child",
					WorkflowID: "child",
					Parameters: []parser.Parameter{
						{Name: "userId", Value: "$inputs.uid"},
					},
				},
			},
		},
		parser.Workflow{
			WorkflowID: "child",
			Steps: []parser.Step{
				{
					StepID:        "get-user",
					OperationPath: "/users/{userId}",
					Parameters: []parser.Parameter{
						{Name: "userId", In: "path", Value: "$inputs.userId"},
					},
					SuccessCriteria: []parser.SuccessCriterion{
						{Condition: "$statusCode == 200"},
					},
				},
			},
		},
	)

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "parent", map[string]any{"uid": "42"})
	if err != nil {
		t.Fatalf("expected no error, got: %v", err)
	}
	if gotPath != "/users/42" {
		t.Fatalf("expected /users/42, got %s", gotPath)
	}
}

func TestExecute_SubWorkflowStepFailure(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
		_, _ = io.WriteString(w, `{"error":"fail"}`)
	}))
	defer ts.Close()

	spec := makeSpec(
		parser.Workflow{
			WorkflowID: "parent",
			Steps: []parser.Step{
				{StepID: "call-child", WorkflowID: "child"},
			},
		},
		parser.Workflow{
			WorkflowID: "child",
			Steps: []parser.Step{
				{
					StepID:        "s1",
					OperationPath: "/fail",
					SuccessCriteria: []parser.SuccessCriterion{
						{Condition: "$statusCode == 200"},
					},
				},
			},
		},
	)

	engine := newTestEngine(ts, spec)
	_, err := engine.Execute(context.Background(), "parent", nil)
	if err == nil {
		t.Fatal("expected error from child workflow failure")
	}
	if !strings.Contains(err.Error(), "sub-workflow child") {
		t.Fatalf("expected sub-workflow error, got: %v", err)
	}
}

func TestExecute_GotoWorkflow(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case "/main":
			w.WriteHeader(500)
		case "/fallback":
			w.WriteHeader(200)
			_, _ = io.WriteString(w, `{"fallback":true}`)
		default:
			w.WriteHeader(404)
		}
	}))
	defer ts.Close()

	spec := makeSpec(
		parser.Workflow{
			WorkflowID: "main-wf",
			Steps: []parser.Step{
				{
					StepID:        "s1",
					OperationPath: "/main",
					SuccessCriteria: []parser.SuccessCriterion{
						{Condition: "$statusCode == 200"},
					},
					OnFailure: []parser.OnAction{
						{Type: "goto", WorkflowID: "fallback-wf"},
					},
				},
			},
		},
		parser.Workflow{
			WorkflowID: "fallback-wf",
			Steps: []parser.Step{
				{
					StepID:        "fb",
					OperationPath: "/fallback",
					SuccessCriteria: []parser.SuccessCriterion{
						{Condition: "$statusCode == 200"},
					},
					Outputs: map[string]string{
						"ok": "$response.body.fallback",
					},
				},
			},
			Outputs: map[string]string{
				"ok": "$steps.fb.outputs.ok",
			},
		},
	)

	engine := newTestEngine(ts, spec)
	outputs, err := engine.Execute(context.Background(), "main-wf", nil)
	if err != nil {
		t.Fatalf("expected no error from goto workflow, got: %v", err)
	}
	if outputs["ok"] != true {
		t.Fatalf("expected ok=true from fallback workflow, got %v", outputs["ok"])
	}
}

func TestExecute_RecursionGuard(t *testing.T) {
	// Workflow A calls Workflow B, which calls Workflow A → infinite recursion
	spec := makeSpec(
		parser.Workflow{
			WorkflowID: "wf-a",
			Steps: []parser.Step{
				{StepID: "call-b", WorkflowID: "wf-b"},
			},
		},
		parser.Workflow{
			WorkflowID: "wf-b",
			Steps: []parser.Step{
				{StepID: "call-a", WorkflowID: "wf-a"},
			},
		},
	)

	engine := NewEngine(spec)
	_, err := engine.Execute(context.Background(), "wf-a", nil)
	if err == nil {
		t.Fatal("expected error from recursion guard")
	}
	if !strings.Contains(err.Error(), "max call depth") {
		t.Fatalf("expected max call depth error, got: %v", err)
	}
}

// ── Trace hook tests ───────────────────────────────────────────────────

type testTraceHook struct {
	beforeEvents []StepEvent
	afterEvents  []StepEvent
}

func (h *testTraceHook) BeforeStep(_ context.Context, event StepEvent) {
	h.beforeEvents = append(h.beforeEvents, event)
}

func (h *testTraceHook) AfterStep(_ context.Context, event StepEvent) {
	h.afterEvents = append(h.afterEvents, event)
}

func TestTraceHook_Invoked(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"ok":true}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{
			{StepID: "s1", OperationPath: "/a", SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}}},
			{StepID: "s2", OperationPath: "/b", SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}}},
		},
	})

	hook := &testTraceHook{}
	engine := newTestEngine(ts, spec)
	engine.SetTraceHook(hook)

	_, err := engine.Execute(context.Background(), "wf", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if len(hook.beforeEvents) != 2 {
		t.Fatalf("expected 2 BeforeStep events, got %d", len(hook.beforeEvents))
	}
	if len(hook.afterEvents) != 2 {
		t.Fatalf("expected 2 AfterStep events, got %d", len(hook.afterEvents))
	}

	// Verify event ordering and content
	if hook.beforeEvents[0].StepID != "s1" || hook.beforeEvents[1].StepID != "s2" {
		t.Fatalf("expected steps [s1, s2], got [%s, %s]", hook.beforeEvents[0].StepID, hook.beforeEvents[1].StepID)
	}
	if hook.afterEvents[0].StatusCode != 200 {
		t.Fatalf("expected status 200, got %d", hook.afterEvents[0].StatusCode)
	}
	if hook.afterEvents[0].Duration <= 0 {
		t.Fatal("expected positive duration")
	}
}

func TestTraceHook_WorkflowID(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(200)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "my-workflow",
		Steps: []parser.Step{
			{StepID: "s1", OperationPath: "/test", SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}}},
		},
	})

	hook := &testTraceHook{}
	engine := newTestEngine(ts, spec)
	engine.SetTraceHook(hook)

	_, _ = engine.Execute(context.Background(), "my-workflow", nil)

	if hook.beforeEvents[0].WorkflowID != "my-workflow" {
		t.Fatalf("expected workflow ID 'my-workflow', got %q", hook.beforeEvents[0].WorkflowID)
	}
}

func TestTraceHook_SubWorkflowStep(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"ok":true}`)
	}))
	defer ts.Close()

	spec := makeSpec(
		parser.Workflow{
			WorkflowID: "parent",
			Steps: []parser.Step{
				{StepID: "call-child", WorkflowID: "child"},
			},
		},
		parser.Workflow{
			WorkflowID: "child",
			Steps: []parser.Step{
				{StepID: "s1", OperationPath: "/api", SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}}},
			},
		},
	)

	hook := &testTraceHook{}
	engine := newTestEngine(ts, spec)
	engine.SetTraceHook(hook)

	_, err := engine.Execute(context.Background(), "parent", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Should see: BeforeStep(call-child in parent), BeforeStep(s1 in child), AfterStep(s1 in child), AfterStep(call-child in parent)
	if len(hook.beforeEvents) != 2 {
		t.Fatalf("expected 2 before events (parent+child), got %d", len(hook.beforeEvents))
	}
	// First event is the sub-workflow call from parent
	if hook.beforeEvents[0].WorkflowIDRef != "child" {
		t.Fatalf("expected first before event to reference 'child', got %q", hook.beforeEvents[0].WorkflowIDRef)
	}
	// Second event is the HTTP step inside child
	if hook.beforeEvents[1].OperationPath != "/api" {
		t.Fatalf("expected second before event with operationPath '/api', got %q", hook.beforeEvents[1].OperationPath)
	}
}

func TestTraceHook_NilSafe(t *testing.T) {
	// Engine without a trace hook should not panic
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(200)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{
			{StepID: "s1", OperationPath: "/test", SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}}},
		},
	})

	engine := newTestEngine(ts, spec)
	// No SetTraceHook call — hook is nil
	_, err := engine.Execute(context.Background(), "wf", nil)
	if err != nil {
		t.Fatalf("unexpected error with nil hook: %v", err)
	}
}

func TestTraceHook_ErrorCapture(t *testing.T) {
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(500)
		_, _ = io.WriteString(w, `{"error":"fail"}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{
			{StepID: "s1", OperationPath: "/fail", SuccessCriteria: []parser.SuccessCriterion{{Condition: "$statusCode == 200"}}},
		},
	})

	hook := &testTraceHook{}
	engine := newTestEngine(ts, spec)
	engine.SetTraceHook(hook)

	_, _ = engine.Execute(context.Background(), "wf", nil)

	if len(hook.afterEvents) != 1 {
		t.Fatalf("expected 1 after event, got %d", len(hook.afterEvents))
	}
	// Step returned 500, but success criteria failed (not a network error)
	// So StatusCode should be captured
	if hook.afterEvents[0].StatusCode != 500 {
		t.Fatalf("expected status 500, got %d", hook.afterEvents[0].StatusCode)
	}
}

// ── operationId resolution tests ────────────────────────────────────────

func TestLoadOpenAPISpec_Basic(t *testing.T) {
	spec := makeSpec(parser.Workflow{WorkflowID: "wf"})
	engine := NewEngine(spec)

	openAPISpec := []byte(`
openapi: "3.0.0"
paths:
  /users:
    get:
      operationId: listUsers
    post:
      operationId: createUser
  /users/{id}:
    get:
      operationId: getUser
    put:
      operationId: updateUser
    delete:
      operationId: deleteUser
`)
	if err := engine.LoadOpenAPISpec(openAPISpec); err != nil {
		t.Fatalf("failed to load OpenAPI spec: %v", err)
	}

	// Verify resolution
	method, path, err := engine.resolveOperationID("listUsers")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if method != "GET" || path != "/users" {
		t.Fatalf("expected GET /users, got %s %s", method, path)
	}

	method, path, err = engine.resolveOperationID("createUser")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if method != "POST" || path != "/users" {
		t.Fatalf("expected POST /users, got %s %s", method, path)
	}

	method, path, err = engine.resolveOperationID("deleteUser")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if method != "DELETE" || path != "/users/{id}" {
		t.Fatalf("expected DELETE /users/{id}, got %s %s", method, path)
	}
}

func TestLoadOpenAPISpec_NotFound(t *testing.T) {
	engine := NewEngine(makeSpec())

	openAPISpec := []byte(`{"openapi":"3.0.0","paths":{"/health":{"get":{"operationId":"healthCheck"}}}}`)
	if err := engine.LoadOpenAPISpec(openAPISpec); err != nil {
		t.Fatalf("failed to load: %v", err)
	}

	_, _, err := engine.resolveOperationID("nonexistent")
	if err == nil {
		t.Fatal("expected error for unknown operationId")
	}
}

func TestLoadOpenAPISpec_SkipsNonHTTPFields(t *testing.T) {
	engine := NewEngine(makeSpec())

	// "parameters" and "summary" at path level should not be treated as HTTP methods
	openAPISpec := []byte(`
openapi: "3.0.0"
paths:
  /items:
    parameters:
      - name: format
    get:
      operationId: listItems
`)
	if err := engine.LoadOpenAPISpec(openAPISpec); err != nil {
		t.Fatalf("failed to load: %v", err)
	}

	method, path, err := engine.resolveOperationID("listItems")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if method != "GET" || path != "/items" {
		t.Fatalf("expected GET /items, got %s %s", method, path)
	}
}

func TestExecute_OperationID(t *testing.T) {
	var gotMethod, gotPath string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotMethod = r.Method
		gotPath = r.URL.Path
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"users":[]}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{{
			StepID:      "s1",
			OperationID: "listUsers",
			SuccessCriteria: []parser.SuccessCriterion{
				{Condition: "$statusCode == 200"},
			},
		}},
	})

	engine := newTestEngine(ts, spec)
	openAPI := []byte(`{"openapi":"3.0.0","paths":{"/users":{"get":{"operationId":"listUsers"}}}}`)
	if err := engine.LoadOpenAPISpec(openAPI); err != nil {
		t.Fatalf("failed to load OpenAPI spec: %v", err)
	}

	_, err := engine.Execute(context.Background(), "wf", nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if gotMethod != "GET" {
		t.Fatalf("expected GET, got %s", gotMethod)
	}
	if gotPath != "/users" {
		t.Fatalf("expected /users, got %s", gotPath)
	}
}

func TestExecute_OperationIDWithPathParams(t *testing.T) {
	var gotPath string
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotPath = r.URL.Path
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{"name":"Alice"}`)
	}))
	defer ts.Close()

	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{{
			StepID:      "s1",
			OperationID: "getUser",
			Parameters: []parser.Parameter{
				{Name: "id", In: "path", Value: "$inputs.userId"},
			},
			SuccessCriteria: []parser.SuccessCriterion{
				{Condition: "$statusCode == 200"},
			},
		}},
	})

	engine := newTestEngine(ts, spec)
	openAPI := []byte(`{"openapi":"3.0.0","paths":{"/users/{id}":{"get":{"operationId":"getUser"}}}}`)
	if err := engine.LoadOpenAPISpec(openAPI); err != nil {
		t.Fatalf("failed to load: %v", err)
	}

	_, err := engine.Execute(context.Background(), "wf", map[string]any{"userId": "42"})
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if gotPath != "/users/42" {
		t.Fatalf("expected /users/42, got %s", gotPath)
	}
}

func TestExecute_OperationIDNotLoaded(t *testing.T) {
	spec := makeSpec(parser.Workflow{
		WorkflowID: "wf",
		Steps: []parser.Step{{
			StepID:      "s1",
			OperationID: "listUsers",
			SuccessCriteria: []parser.SuccessCriterion{
				{Condition: "$statusCode == 200"},
			},
		}},
	})

	engine := NewEngine(spec)
	_, err := engine.Execute(context.Background(), "wf", nil)
	if err == nil {
		t.Fatal("expected error for unresolved operationId")
	}
	if !strings.Contains(err.Error(), "operationId") {
		t.Fatalf("expected operationId error, got: %v", err)
	}
}

func TestExecute_SubWorkflowNotFound(t *testing.T) {
	spec := makeSpec(
		parser.Workflow{
			WorkflowID: "parent",
			Steps: []parser.Step{
				{StepID: "call-missing", WorkflowID: "nonexistent"},
			},
		},
	)

	engine := NewEngine(spec)
	_, err := engine.Execute(context.Background(), "parent", nil)
	if err == nil {
		t.Fatal("expected error for nonexistent sub-workflow")
	}
	if !strings.Contains(err.Error(), `workflow "nonexistent" not found`) {
		t.Fatalf("expected workflow not found error, got: %v", err)
	}
}
