package runtime

import (
	"context"
	"io"
	"net/http"
	"net/url"
	"net/http/httptest"
	"strings"
	"testing"

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
	expected := "goto: no stepId specified"
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
