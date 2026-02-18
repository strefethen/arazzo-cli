// Package runtime provides the Arazzo workflow execution engine.
package runtime

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
	"regexp"
	"strings"

	"github.com/strefethen/arazzo-cli/parser"
)

// maxRetriesPerStep limits retry attempts to prevent infinite loops.
const maxRetriesPerStep = 3

// Package-level compiled regex patterns for performance.
var (
	pathParamRe  = regexp.MustCompile(`\{([^}]+)\}`)
	arrayIndexRe = regexp.MustCompile(`\[(\d+)\]`)
)

// stepResult captures the outcome of executing a step.
type stepResult struct {
	success  bool      // true if success criteria passed
	response *Response // HTTP response (may be nil for network errors)
	err      error     // network/system error (not API error)
}

// Engine executes Arazzo workflows at runtime without code generation.
type Engine struct {
	client        *Client
	spec          *parser.ArazzoSpec
	baseURL       string                    // from first sourceDescription
	workflowIndex map[string]int            // workflowID → index in spec.Workflows
	stepIndexes   map[string]map[string]int // workflowID → (stepID → index in Steps)
}

// NewEngine creates an Engine from a parsed Arazzo spec.
func NewEngine(spec *parser.ArazzoSpec, opts ...ClientOption) *Engine {
	e := &Engine{
		client: NewClient(opts...),
		spec:   spec,
	}
	// Extract base URL from first source description
	if len(spec.SourceDescriptions) > 0 {
		e.baseURL = spec.SourceDescriptions[0].URL
	}
	// Build indexes for O(1) workflow and step lookups
	e.workflowIndex = make(map[string]int, len(spec.Workflows))
	e.stepIndexes = make(map[string]map[string]int, len(spec.Workflows))
	for i, wf := range spec.Workflows {
		e.workflowIndex[wf.WorkflowID] = i
		e.stepIndexes[wf.WorkflowID] = make(map[string]int, len(wf.Steps))
		for j, step := range wf.Steps {
			e.stepIndexes[wf.WorkflowID][step.StepID] = j
		}
	}
	return e
}

// Workflows returns list of available workflow IDs in this spec.
func (e *Engine) Workflows() []string {
	ids := make([]string, len(e.spec.Workflows))
	for i, wf := range e.spec.Workflows {
		ids[i] = wf.WorkflowID
	}
	return ids
}

// GetWorkflow returns a workflow by ID, or nil if not found.
func (e *Engine) GetWorkflow(workflowID string) *parser.Workflow {
	if idx, ok := e.workflowIndex[workflowID]; ok {
		return &e.spec.Workflows[idx]
	}
	return nil
}

// Spec returns the underlying Arazzo spec.
func (e *Engine) Spec() *parser.ArazzoSpec {
	return e.spec
}

// Execute runs a workflow with the given inputs and returns its outputs.
func (e *Engine) Execute(ctx context.Context, workflowID string, inputs map[string]any) (map[string]any, error) {
	// 1. Find workflow by ID
	workflow := e.GetWorkflow(workflowID)
	if workflow == nil {
		return nil, fmt.Errorf("workflow %q not found", workflowID)
	}

	// 2. Initialize variable store with inputs
	vars := NewVarStore()
	for k, v := range inputs {
		vars.SetInput(k, v)
	}

	// 3. Execute steps with onSuccess/onFailure handling
	stepIndex := 0
	retryCount := make(map[int]int) // stepIndex -> retry count
	maxIterations := len(workflow.Steps) * 10 // prevent infinite loops

	for iterations := 0; iterations < maxIterations; iterations++ {
		if stepIndex >= len(workflow.Steps) {
			break // normal completion
		}

		step := workflow.Steps[stepIndex]
		result := e.executeStepWithResult(ctx, step, vars)

		nextIndex, done, err := e.handleStepResult(workflow, stepIndex, result, vars, retryCount)
		if err != nil {
			return nil, err
		}
		if done {
			break
		}

		// Track retries or reset counter when moving to different step
		if nextIndex == stepIndex {
			retryCount[stepIndex]++
		} else {
			delete(retryCount, stepIndex) // moving on, reset
		}
		stepIndex = nextIndex
	}

	// 4. Build outputs from workflow.Outputs expressions
	return e.buildOutputs(workflow, vars), nil
}

// executeStepWithResult runs a single step and returns a result struct.
func (e *Engine) executeStepWithResult(ctx context.Context, step parser.Step, vars *VarStore) stepResult {
	// 1. Build URL from operationPath + parameters
	url := e.buildURL(step, vars)

	// 2. Determine HTTP method (default GET, check for requestBody to use POST)
	method := "GET"
	var body []byte
	if step.RequestBody != nil {
		method = "POST"
		if payload := step.RequestBody.Payload; payload != nil {
			// Evaluate expressions in the payload, then marshal to JSON
			eval := NewExpressionEvaluator(vars)
			resolved := resolvePayload(payload, eval)

			var err error
			body, err = json.Marshal(resolved)
			if err != nil {
				return stepResult{success: false, response: nil, err: fmt.Errorf("marshaling request body: %w", err)}
			}
		}
	}

	// 3. Build headers from header parameters
	headers := make(map[string]string)
	if body != nil {
		headers["Content-Type"] = "application/json"
	}
	eval := NewExpressionEvaluator(vars)
	for _, param := range step.Parameters {
		if param.In == "header" {
			headers[param.Name] = eval.EvaluateString(param.Value)
		}
	}

	// 4. Execute HTTP request
	resp, err := e.client.Request(ctx, RequestConfig{
		Method:  method,
		URL:     url,
		Headers: headers,
		Body:    body,
	})
	if err != nil {
		return stepResult{success: false, response: nil, err: err}
	}

	// 5. Evaluate success criteria
	evalWithResp := NewExpressionEvaluator(vars).WithResponse(resp)
	for _, criterion := range step.SuccessCriteria {
		if !evalWithResp.EvaluateCondition(criterion.Condition) {
			return stepResult{success: false, response: resp, err: nil}
		}
	}

	// 6. Extract outputs
	for name, expr := range step.Outputs {
		var value any
		if strings.HasPrefix(expr, "/") {
			// XPath expression (starts with / or //)
			value = resp.Extract(expr)
		} else if strings.HasPrefix(expr, "$response.header.") || strings.HasPrefix(expr, "$statusCode") {
			// Expressions the evaluator handles directly
			value = evalWithResp.Evaluate(expr)
		} else {
			// Body extraction — convert Arazzo expression to gjson-compatible path
			gjsonPath := toGJSONPath(expr)
			value = evalWithResp.Evaluate("$response.body." + gjsonPath)
		}
		vars.SetStepOutput(step.StepID, name, value)
	}

	return stepResult{success: true, response: resp, err: nil}
}

// handleStepResult processes the outcome of a step and determines what to do next.
func (e *Engine) handleStepResult(wf *parser.Workflow, stepIdx int, result stepResult, vars *VarStore, retryCount map[int]int) (nextIdx int, done bool, err error) {
	step := wf.Steps[stepIdx]

	if result.success {
		// Check onSuccess actions
		action := e.findMatchingAction(step.OnSuccess, vars, result.response)
		if action == nil {
			return stepIdx + 1, false, nil // default: next step
		}
		return e.executeAction(wf, action, stepIdx, false, retryCount)
	}

	// Failure path
	action := e.findMatchingAction(step.OnFailure, vars, result.response)
	if action == nil {
		// No handler = workflow fails
		if result.err != nil {
			return 0, true, fmt.Errorf("step %s: %w", step.StepID, result.err)
		}
		// Include response body in error for debugging
		bodyPreview := string(result.response.Body)
		if len(bodyPreview) > 500 {
			bodyPreview = bodyPreview[:500] + "..."
		}
		return 0, true, fmt.Errorf("step %s: success criteria not met (status=%d, body=%s)", step.StepID, result.response.StatusCode, bodyPreview)
	}
	return e.executeAction(wf, action, stepIdx, true, retryCount)
}

// findMatchingAction returns the first action whose criteria match, or nil.
func (e *Engine) findMatchingAction(actions []parser.OnAction, vars *VarStore, resp *Response) *parser.OnAction {
	eval := NewExpressionEvaluator(vars)
	if resp != nil {
		eval = eval.WithResponse(resp)
	}

	for i := range actions {
		action := &actions[i]
		if len(action.Criteria) == 0 {
			return action // no criteria = always matches
		}
		// All criteria must match
		allMatch := true
		for _, criterion := range action.Criteria {
			if !eval.EvaluateCondition(criterion.Condition) {
				allMatch = false
				break
			}
		}
		if allMatch {
			return action
		}
	}
	return nil
}

// executeAction handles goto/end/retry actions.
func (e *Engine) executeAction(wf *parser.Workflow, action *parser.OnAction, currentIdx int, isFailurePath bool, retryCount map[int]int) (nextIdx int, done bool, err error) {
	switch action.Type {
	case "end":
		if isFailurePath {
			return 0, true, fmt.Errorf("step %s: workflow ended by onFailure action", wf.Steps[currentIdx].StepID)
		}
		return 0, true, nil // success termination
	case "goto":
		if action.StepID != "" {
			idx := e.findStepIndex(wf, action.StepID)
			if idx < 0 {
				return 0, true, fmt.Errorf("goto: step %q not found", action.StepID)
			}
			return idx, false, nil
		}
		// TODO: action.WorkflowID for sub-workflow calls (future)
		return 0, true, fmt.Errorf("goto: no stepId specified")
	case "retry":
		if retryCount[currentIdx] >= maxRetriesPerStep {
			return 0, true, fmt.Errorf("step %s: max retries (%d) exceeded", wf.Steps[currentIdx].StepID, maxRetriesPerStep)
		}
		return currentIdx, false, nil // re-execute same step
	default:
		return currentIdx + 1, false, nil
	}
}

// findStepIndex returns the index of a step by ID, or -1 if not found.
func (e *Engine) findStepIndex(wf *parser.Workflow, stepID string) int {
	if stepIndex, ok := e.stepIndexes[wf.WorkflowID]; ok {
		if idx, found := stepIndex[stepID]; found {
			return idx
		}
	}
	return -1
}

// buildOutputs constructs the workflow output map from expressions.
func (e *Engine) buildOutputs(workflow *parser.Workflow, vars *VarStore) map[string]any {
	outputs := make(map[string]any)
	eval := NewExpressionEvaluator(vars)
	for name, expr := range workflow.Outputs {
		outputs[name] = eval.Evaluate(expr)
	}
	return outputs
}

// buildURL constructs the full URL for a step, resolving path parameters.
func (e *Engine) buildURL(step parser.Step, vars *VarStore) string {
	// Start with base URL + operation path (unless operationPath is absolute)
	var target string
	if strings.HasPrefix(step.OperationPath, "http://") || strings.HasPrefix(step.OperationPath, "https://") {
		target = step.OperationPath
	} else {
		target = e.baseURL + step.OperationPath
	}

	// Build parameter map
	eval := NewExpressionEvaluator(vars)
	pathParams := make(map[string]any)
	queryParams := make(map[string]string)

	for _, param := range step.Parameters {
		value := eval.Evaluate(param.Value)
		switch param.In {
		case "path":
			pathParams[param.Name] = value
		case "query":
			if value != nil {
				queryParams[param.Name] = fmt.Sprintf("%v", value)
			}
		}
	}

	// Replace {param} placeholders with values
	target = pathParamRe.ReplaceAllStringFunc(target, func(match string) string {
		name := match[1 : len(match)-1]
		if val, ok := pathParams[name]; ok {
			return fmt.Sprintf("%v", val)
		}
		return match
	})

	// Append query parameters (URL-encoded)
	if len(queryParams) > 0 {
		qv := make(url.Values, len(queryParams))
		for k, v := range queryParams {
			qv.Set(k, v)
		}
		target += "?" + qv.Encode()
	}

	return target
}

// resolvePayload recursively walks a parsed YAML payload and evaluates
// any Arazzo expressions ($inputs.*, $steps.*) found in string values.
func resolvePayload(payload any, eval *ExpressionEvaluator) any {
	switch v := payload.(type) {
	case map[string]any:
		out := make(map[string]any, len(v))
		for k, val := range v {
			out[k] = resolvePayload(val, eval)
		}
		return out
	case []any:
		out := make([]any, len(v))
		for i, val := range v {
			out[i] = resolvePayload(val, eval)
		}
		return out
	case string:
		if strings.HasPrefix(v, "$") {
			return eval.Evaluate(v)
		}
		return v
	default:
		return v
	}
}

// toGJSONPath converts Arazzo expression to gjson path.
// - Strips $response.body prefix if present (with or without trailing dot)
// - Converts [N] array syntax to .N for gjson
func toGJSONPath(expr string) string {
	// Strip $response.body. or $response.body (for array access like [0])
	path := strings.TrimPrefix(expr, "$response.body.")
	if path == expr {
		// Didn't match with dot, try without dot (for $response.body[0])
		path = strings.TrimPrefix(expr, "$response.body")
	}
	// Convert [N] to .N for gjson
	result := arrayIndexRe.ReplaceAllString(path, ".$1")
	// Remove leading dot if present (from array at root like [0].q -> .0.q)
	return strings.TrimPrefix(result, ".")
}
