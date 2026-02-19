// Package runtime provides the Arazzo workflow execution engine.
package runtime

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/strefethen/arazzo-cli/parser"
	"gopkg.in/yaml.v3"
)

// maxRetriesPerStep limits retry attempts to prevent infinite loops.
const maxRetriesPerStep = 3

// maxCallDepth limits sub-workflow recursion to prevent stack overflow.
const maxCallDepth = 10

// ctxKeyCallDepth is the context key for tracking sub-workflow call depth.
type ctxKeyCallDepth struct{}

// callDepth returns the current sub-workflow call depth from context.
func callDepth(ctx context.Context) int {
	if v, ok := ctx.Value(ctxKeyCallDepth{}).(int); ok {
		return v
	}
	return 0
}

// withCallDepth returns a new context with the call depth incremented by 1.
func withCallDepth(ctx context.Context) context.Context {
	return context.WithValue(ctx, ctxKeyCallDepth{}, callDepth(ctx)+1)
}

// validMethods is the set of valid HTTP methods for parseMethod.
var validMethods = map[string]bool{
	"GET": true, "POST": true, "PUT": true, "PATCH": true,
	"DELETE": true, "HEAD": true, "OPTIONS": true,
}

// StepEvent provides information about a step execution for tracing hooks.
type StepEvent struct {
	WorkflowID    string
	StepID        string
	OperationPath string // empty for sub-workflow steps
	WorkflowIDRef string // non-empty for sub-workflow steps
	StatusCode    int    // HTTP status (0 for sub-workflows or errors)
	Outputs       map[string]any
	Err           error
	Duration      time.Duration
}

// TraceHook allows callers to observe workflow execution.
// Implementations must be safe for concurrent use when parallel mode is enabled.
type TraceHook interface {
	BeforeStep(ctx context.Context, event StepEvent)
	AfterStep(ctx context.Context, event StepEvent)
}

// stepResult captures the outcome of executing a step.
type stepResult struct {
	success  bool      // true if success criteria passed
	response *Response // HTTP response (may be nil for network errors)
	err      error     // network/system error (not API error)
}

// operationEntry maps an operationId to its HTTP method and path.
type operationEntry struct {
	Method string
	Path   string
}

// Engine executes Arazzo workflows at runtime without code generation.
type Engine struct {
	client        *Client
	spec          *parser.ArazzoSpec
	baseURL       string                    // from first sourceDescription
	workflowIndex map[string]int            // workflowID → index in spec.Workflows
	stepIndexes   map[string]map[string]int // workflowID → (stepID → index in Steps)
	traceHook     TraceHook                 // optional execution observer
	opIndex       map[string]operationEntry // operationId → {method, path}
	parallelMode  bool                      // execute independent steps concurrently
}

// SetTraceHook sets an optional hook for observing step execution.
func (e *Engine) SetTraceHook(hook TraceHook) {
	e.traceHook = hook
}

// SetParallelMode enables or disables parallel execution of independent steps.
func (e *Engine) SetParallelMode(enabled bool) {
	e.parallelMode = enabled
}

// LoadOpenAPISpec parses an OpenAPI 3.x spec (JSON or YAML) and builds
// an operationId → (method, path) index for operationId resolution.
func (e *Engine) LoadOpenAPISpec(data []byte) error {
	var spec struct {
		Paths map[string]map[string]any `yaml:"paths" json:"paths"`
	}
	if err := yaml.Unmarshal(data, &spec); err != nil {
		return fmt.Errorf("parsing OpenAPI spec: %w", err)
	}

	httpMethods := map[string]bool{
		"get": true, "post": true, "put": true, "patch": true,
		"delete": true, "head": true, "options": true, "trace": true,
	}

	for path, methods := range spec.Paths {
		for method, opAny := range methods {
			if !httpMethods[strings.ToLower(method)] {
				continue // skip non-HTTP fields like "parameters", "summary"
			}
			opMap, ok := opAny.(map[string]any)
			if !ok {
				continue
			}
			if opID, ok := opMap["operationId"].(string); ok && opID != "" {
				e.opIndex[opID] = operationEntry{
					Method: strings.ToUpper(method),
					Path:   path,
				}
			}
		}
	}
	return nil
}

// resolveOperationID looks up an operationId and returns (method, path).
func (e *Engine) resolveOperationID(operationID string) (string, string, error) {
	entry, ok := e.opIndex[operationID]
	if !ok {
		return "", "", fmt.Errorf("operationId %q not found in loaded OpenAPI specs", operationID)
	}
	return entry.Method, entry.Path, nil
}

// NewEngine creates an Engine from a parsed Arazzo spec.
func NewEngine(spec *parser.ArazzoSpec, opts ...ClientOption) *Engine {
	e := &Engine{
		client:  NewClient(opts...),
		spec:    spec,
		opIndex: make(map[string]operationEntry),
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
	// Check recursion depth for sub-workflow calls
	if callDepth(ctx) >= maxCallDepth {
		return nil, fmt.Errorf("max call depth (%d) exceeded calling workflow %q", maxCallDepth, workflowID)
	}

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

	// 2b. Parallel path: run independent steps concurrently when enabled
	if e.parallelMode && !hasControlFlow(workflow) {
		return e.executeParallel(ctx, workflowID, workflow, vars)
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

		// Trace: before step
		if e.traceHook != nil {
			e.traceHook.BeforeStep(ctx, StepEvent{
				WorkflowID:    workflowID,
				StepID:        step.StepID,
				OperationPath: step.OperationPath,
				WorkflowIDRef: step.WorkflowID,
			})
		}

		start := time.Now()
		result := e.executeStepWithResult(ctx, step, vars)
		duration := time.Since(start)

		// Trace: after step
		if e.traceHook != nil {
			event := StepEvent{
				WorkflowID:    workflowID,
				StepID:        step.StepID,
				OperationPath: step.OperationPath,
				WorkflowIDRef: step.WorkflowID,
				Err:           result.err,
				Duration:      duration,
				Outputs:       vars.GetStepOutputs(step.StepID),
			}
			if result.response != nil {
				event.StatusCode = result.response.StatusCode
			}
			e.traceHook.AfterStep(ctx, event)
		}

		nextIndex, done, gotoWorkflowID, err := e.handleStepResult(ctx, workflow, stepIndex, result, vars, retryCount)
		if err != nil {
			return nil, err
		}
		if gotoWorkflowID != "" {
			return e.Execute(withCallDepth(ctx), gotoWorkflowID, vars.GetInputs())
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
	// Sub-workflow step: call another workflow instead of making an HTTP request
	if step.WorkflowID != "" {
		return e.executeSubWorkflowStep(ctx, step, vars)
	}

	// Resolve operationId to method + path if needed
	if step.OperationID != "" && step.OperationPath == "" {
		method, path, err := e.resolveOperationID(step.OperationID)
		if err != nil {
			return stepResult{success: false, err: err}
		}
		step.OperationPath = method + " " + path
	}

	// 1. Parse method + build URL (single parseMethod call)
	explicitMethod, opPath := parseMethod(step.OperationPath)
	url := e.buildURLFromPath(opPath, step, vars)

	// 2. Determine HTTP method
	method := explicitMethod
	var body []byte
	if method == "" {
		// No explicit method — infer from body presence
		if step.RequestBody != nil {
			method = "POST"
		} else {
			method = "GET"
		}
	}
	// Reuse a single evaluator for payload, headers, and criteria
	eval := NewExpressionEvaluator(vars)

	if step.RequestBody != nil {
		if payload := step.RequestBody.Payload; payload != nil {
			// Evaluate expressions in the payload, then marshal to JSON
			resolved := resolvePayload(payload, eval)

			var err error
			body, err = json.Marshal(resolved)
			if err != nil {
				return stepResult{success: false, err: fmt.Errorf("marshaling request body: %w", err)}
			}
		}
	}

	// 3. Build headers from header parameters
	headers := make(map[string]string)
	if body != nil {
		ct := "application/json"
		if step.RequestBody != nil && step.RequestBody.ContentType != "" {
			ct = step.RequestBody.ContentType
		}
		headers["Content-Type"] = ct
	}
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
		return stepResult{success: false, err: err}
	}

	// 5. Evaluate success criteria (reuse evaluator, add response context)
	eval.WithResponse(resp)
	for _, criterion := range step.SuccessCriteria {
		if !evaluateCriterion(criterion, eval, resp) {
			return stepResult{success: false, response: resp}
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
			value = eval.Evaluate(expr)
		} else {
			// Body extraction — convert Arazzo expression to gjson-compatible path
			gjsonPath := toGJSONPath(expr)
			value = eval.Evaluate("$response.body." + gjsonPath)
		}
		vars.SetStepOutput(step.StepID, name, value)
	}

	return stepResult{success: true, response: resp}
}

// executeSubWorkflowStep runs a sub-workflow as a step, propagating inputs and outputs.
func (e *Engine) executeSubWorkflowStep(ctx context.Context, step parser.Step, vars *VarStore) stepResult {
	// Build sub-workflow inputs from step parameters
	eval := NewExpressionEvaluator(vars)
	subInputs := make(map[string]any)
	for _, param := range step.Parameters {
		subInputs[param.Name] = eval.Evaluate(param.Value)
	}

	// Execute sub-workflow recursively with incremented call depth
	outputs, err := e.Execute(withCallDepth(ctx), step.WorkflowID, subInputs)
	if err != nil {
		return stepResult{success: false, err: fmt.Errorf("sub-workflow %s: %w", step.WorkflowID, err)}
	}

	// Store sub-workflow outputs under this step's ID
	for name, value := range outputs {
		vars.SetStepOutput(step.StepID, name, value)
	}

	// Evaluate success criteria (variable-based only, no response context)
	evalPost := NewExpressionEvaluator(vars)
	for _, criterion := range step.SuccessCriteria {
		if !evaluateCriterion(criterion, evalPost, nil) {
			return stepResult{success: false}
		}
	}

	return stepResult{success: true}
}

// handleStepResult processes the outcome of a step and determines what to do next.
// Returns gotoWorkflowID if control should transfer to another workflow.
func (e *Engine) handleStepResult(ctx context.Context, wf *parser.Workflow, stepIdx int, result stepResult, vars *VarStore, retryCount map[int]int) (nextIdx int, done bool, gotoWorkflowID string, err error) {
	step := wf.Steps[stepIdx]

	if result.success {
		// Check onSuccess actions
		action := e.findMatchingAction(step.OnSuccess, vars, result.response)
		if action == nil {
			return stepIdx + 1, false, "", nil // default: next step
		}
		return e.executeAction(ctx, wf, action, stepIdx, false, retryCount)
	}

	// Failure path
	action := e.findMatchingAction(step.OnFailure, vars, result.response)
	if action == nil {
		// No handler = workflow fails
		if result.err != nil {
			return 0, true, "", fmt.Errorf("step %s: %w", step.StepID, result.err)
		}
		// Include response body in error for debugging
		bodyPreview := string(result.response.Body)
		if len(bodyPreview) > 500 {
			bodyPreview = bodyPreview[:500] + "..."
		}
		return 0, true, "", fmt.Errorf("step %s: success criteria not met (status=%d, body=%s)", step.StepID, result.response.StatusCode, bodyPreview)
	}
	return e.executeAction(ctx, wf, action, stepIdx, true, retryCount)
}

// findMatchingAction returns the first action whose criteria match, or nil.
func (e *Engine) findMatchingAction(actions []parser.OnAction, vars *VarStore, resp *Response) *parser.OnAction {
	eval := NewExpressionEvaluator(vars)
	if resp != nil {
		eval.WithResponse(resp)
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
// Returns gotoWorkflowID if the action transfers control to another workflow.
func (e *Engine) executeAction(ctx context.Context, wf *parser.Workflow, action *parser.OnAction, currentIdx int, isFailurePath bool, retryCount map[int]int) (nextIdx int, done bool, gotoWorkflowID string, err error) {
	switch action.Type {
	case "end":
		if isFailurePath {
			return 0, true, "", fmt.Errorf("step %s: workflow ended by onFailure action", wf.Steps[currentIdx].StepID)
		}
		return 0, true, "", nil // success termination
	case "goto":
		if action.StepID != "" {
			idx := e.findStepIndex(wf, action.StepID)
			if idx < 0 {
				return 0, true, "", fmt.Errorf("goto: step %q not found", action.StepID)
			}
			return idx, false, "", nil
		}
		if action.WorkflowID != "" {
			return 0, true, action.WorkflowID, nil
		}
		return 0, true, "", fmt.Errorf("goto: no stepId or workflowId specified")
	case "retry":
		limit := maxRetriesPerStep
		if action.RetryLimit > 0 {
			limit = action.RetryLimit
		}
		if retryCount[currentIdx] >= limit {
			return 0, true, "", fmt.Errorf("step %s: max retries (%d) exceeded", wf.Steps[currentIdx].StepID, limit)
		}
		if action.RetryAfter > 0 {
			select {
			case <-time.After(time.Duration(action.RetryAfter) * time.Second):
			case <-ctx.Done():
				return 0, true, "", ctx.Err()
			}
		}
		return currentIdx, false, "", nil // re-execute same step
	default:
		return currentIdx + 1, false, "", nil
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
	_, opPath := parseMethod(step.OperationPath)
	return e.buildURLFromPath(opPath, step, vars)
}

// buildURLFromPath constructs the full URL using a pre-parsed operation path.
func (e *Engine) buildURLFromPath(opPath string, step parser.Step, vars *VarStore) string {
	// Start with base URL + operation path (unless operationPath is absolute)
	var target string
	if strings.HasPrefix(opPath, "http://") || strings.HasPrefix(opPath, "https://") {
		target = opPath
	} else {
		target = strings.TrimRight(e.baseURL, "/") + opPath
	}

	// Build parameter maps
	eval := NewExpressionEvaluator(vars)
	pathParams := make(map[string]string)
	queryParams := make(map[string]string)

	for _, param := range step.Parameters {
		value := eval.Evaluate(param.Value)
		switch param.In {
		case "path":
			pathParams[param.Name] = anyToString(value)
		case "query":
			if value != nil {
				queryParams[param.Name] = anyToString(value)
			}
		}
	}

	// Replace {param} placeholders with a single-pass scan (no regex)
	if len(pathParams) > 0 && strings.ContainsRune(target, '{') {
		target = replacePathParams(target, pathParams)
	}

	// Append query parameters (manual encoding avoids url.Values key sorting)
	if len(queryParams) > 0 {
		var b strings.Builder
		b.WriteString(target)
		b.WriteByte('?')
		first := true
		for k, v := range queryParams {
			if !first {
				b.WriteByte('&')
			}
			b.WriteString(url.QueryEscape(k))
			b.WriteByte('=')
			b.WriteString(url.QueryEscape(v))
			first = false
		}
		target = b.String()
	}

	return target
}

// replacePathParams replaces {name} placeholders in a URL path using a single-pass scan.
func replacePathParams(path string, params map[string]string) string {
	var b strings.Builder
	b.Grow(len(path))
	for {
		open := strings.IndexByte(path, '{')
		if open < 0 {
			b.WriteString(path)
			break
		}
		close := strings.IndexByte(path[open+1:], '}')
		if close < 0 {
			b.WriteString(path)
			break
		}
		close += open + 1 // absolute index
		name := path[open+1 : close]
		b.WriteString(path[:open])
		if val, ok := params[name]; ok {
			b.WriteString(val)
		} else {
			b.WriteString(path[open : close+1]) // keep {name} as-is
		}
		path = path[close+1:]
	}
	return b.String()
}

// anyToString converts a value to string without fmt.Sprintf reflection overhead.
func anyToString(v any) string {
	switch t := v.(type) {
	case string:
		return t
	case float64:
		return strconv.FormatFloat(t, 'f', -1, 64)
	case int:
		return strconv.Itoa(t)
	case int64:
		return strconv.FormatInt(t, 10)
	case bool:
		return strconv.FormatBool(t)
	default:
		return fmt.Sprintf("%v", v)
	}
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

// regexCache stores compiled regex patterns to avoid recompilation.
var regexCache sync.Map // string -> *regexp.Regexp

// getCompiledRegex returns a cached compiled regex, compiling on first access.
func getCompiledRegex(pattern string) (*regexp.Regexp, error) {
	if cached, ok := regexCache.Load(pattern); ok {
		return cached.(*regexp.Regexp), nil
	}
	re, err := regexp.Compile(pattern)
	if err != nil {
		return nil, err
	}
	regexCache.Store(pattern, re)
	return re, nil
}

// evaluateCriterion dispatches criterion evaluation based on its type.
// Supported types: "simple" (default), "regex", "jsonpath".
func evaluateCriterion(c parser.SuccessCriterion, eval *ExpressionEvaluator, resp *Response) bool {
	switch c.Type {
	case "regex":
		// Context expression provides the value to match against.
		contextVal := eval.EvaluateString(c.Context)
		re, err := getCompiledRegex(c.Condition)
		if err != nil {
			return false
		}
		return re.MatchString(contextVal)

	case "jsonpath":
		// Context resolves to the JSON body; condition is a gjson query.
		// A truthy result (exists and is not false/0/empty) means success.
		if resp == nil {
			return false
		}
		result := resp.Extract(c.Condition)
		return result != nil && result != false && result != 0.0 && result != ""

	default: // "simple" or empty
		return eval.EvaluateCondition(c.Condition)
	}
}

// parseMethod extracts an HTTP method prefix from operationPath.
// Supports "METHOD /path" format (e.g., "PUT /users/{id}").
// Returns ("", operationPath) if no method prefix is present.
func parseMethod(operationPath string) (method, path string) {
	idx := strings.IndexByte(operationPath, ' ')
	if idx > 0 && idx <= 7 { // longest method is "OPTIONS" (7 chars)
		candidate := operationPath[:idx]
		if validMethods[candidate] {
			return candidate, operationPath[idx+1:]
		}
	}
	return "", operationPath
}

// toGJSONPath converts an Arazzo expression to a gjson path.
// Strips $response.body prefix and converts [N] array syntax to .N for gjson.
func toGJSONPath(expr string) string {
	// Strip $response.body. or $response.body (for array access like [0])
	path, found := strings.CutPrefix(expr, "$response.body.")
	if !found {
		path, _ = strings.CutPrefix(expr, "$response.body")
	}

	// Fast path: no brackets means no array indexing to convert
	if !strings.ContainsRune(path, '[') {
		return path
	}

	// Convert [N] to .N for gjson with a single-pass scan (no regex)
	var b strings.Builder
	b.Grow(len(path))
	for i := 0; i < len(path); i++ {
		if path[i] == '[' {
			if end := strings.IndexByte(path[i+1:], ']'); end >= 0 {
				if b.Len() > 0 || i > 0 {
					b.WriteByte('.')
				}
				b.WriteString(path[i+1 : i+1+end])
				i += end + 1 // skip past ']'
				continue
			}
		}
		b.WriteByte(path[i])
	}
	return b.String()
}
