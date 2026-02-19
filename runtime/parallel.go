package runtime

import (
	"context"
	"fmt"
	"regexp"
	"strings"
	"time"

	"github.com/strefethen/arazzo-cli/parser"
	"golang.org/x/sync/errgroup"
)

// stepRefPattern matches $steps.<stepId>. in expression strings.
var stepRefPattern = regexp.MustCompile(`\$steps\.([a-zA-Z_][a-zA-Z0-9_-]*)\.`)

// executionLevel is a group of step indices that can run concurrently.
type executionLevel struct {
	Steps []int
}

// extractStepRefs scans all expression-bearing fields on a step and returns
// the set of stepIDs referenced via $steps.X patterns.
func extractStepRefs(step parser.Step) []string {
	seen := make(map[string]bool)

	scan := func(s string) {
		for _, m := range stepRefPattern.FindAllStringSubmatch(s, -1) {
			seen[m[1]] = true
		}
	}

	scan(step.OperationPath)

	for _, p := range step.Parameters {
		scan(p.Value)
	}

	if step.RequestBody != nil && step.RequestBody.Payload != nil {
		scanPayloadRefs(step.RequestBody.Payload, scan)
	}

	for _, c := range step.SuccessCriteria {
		scan(c.Condition)
		scan(c.Context)
	}

	for _, expr := range step.Outputs {
		scan(expr)
	}

	for _, a := range step.OnSuccess {
		for _, c := range a.Criteria {
			scan(c.Condition)
		}
	}
	for _, a := range step.OnFailure {
		for _, c := range a.Criteria {
			scan(c.Condition)
		}
	}

	result := make([]string, 0, len(seen))
	for id := range seen {
		result = append(result, id)
	}
	return result
}

// scanPayloadRefs recursively walks a payload and calls scan on any string
// value that starts with $.
func scanPayloadRefs(payload any, scan func(string)) {
	switch v := payload.(type) {
	case map[string]any:
		for _, val := range v {
			scanPayloadRefs(val, scan)
		}
	case []any:
		for _, val := range v {
			scanPayloadRefs(val, scan)
		}
	case string:
		if strings.HasPrefix(v, "$") {
			scan(v)
		}
	}
}

// hasControlFlow returns true if any step uses goto, retry, or end actions.
// Workflows with control flow cannot be parallelized.
func hasControlFlow(wf *parser.Workflow) bool {
	for i := range wf.Steps {
		for _, a := range wf.Steps[i].OnSuccess {
			if a.Type == "goto" || a.Type == "retry" || a.Type == "end" {
				return true
			}
		}
		for _, a := range wf.Steps[i].OnFailure {
			if a.Type == "goto" || a.Type == "retry" || a.Type == "end" {
				return true
			}
		}
	}
	return false
}

// buildLevels groups workflow steps into topological execution levels.
// Steps in the same level have no dependencies on each other and can run concurrently.
func buildLevels(wf *parser.Workflow) ([]executionLevel, error) {
	n := len(wf.Steps)
	stepIDToIndex := make(map[string]int, n)
	for i, s := range wf.Steps {
		stepIDToIndex[s.StepID] = i
	}

	// Build dependency sets: deps[i] = set of step indices that step i depends on
	deps := make([]map[int]bool, n)
	for i, step := range wf.Steps {
		refs := extractStepRefs(step)
		deps[i] = make(map[int]bool, len(refs))
		for _, ref := range refs {
			if j, ok := stepIDToIndex[ref]; ok {
				deps[i][j] = true
			}
		}
	}

	// Kahn's algorithm: compute in-degree and find levels
	inDegree := make([]int, n)
	for i := range deps {
		inDegree[i] = len(deps[i])
	}

	var levels []executionLevel
	remaining := n
	assigned := make([]bool, n)

	for remaining > 0 {
		var level executionLevel
		for i := 0; i < n; i++ {
			if !assigned[i] && inDegree[i] == 0 {
				level.Steps = append(level.Steps, i)
			}
		}
		if len(level.Steps) == 0 {
			return nil, fmt.Errorf("dependency cycle detected in workflow %q", wf.WorkflowID)
		}
		for _, idx := range level.Steps {
			assigned[idx] = true
			remaining--
			for i := 0; i < n; i++ {
				if deps[i][idx] {
					delete(deps[i], idx)
					inDegree[i]--
				}
			}
		}
		levels = append(levels, level)
	}

	return levels, nil
}

// executeParallel runs a workflow using level-based parallelism.
// Steps within each level execute concurrently via errgroup.
func (e *Engine) executeParallel(ctx context.Context, workflowID string, wf *parser.Workflow, vars *VarStore) (map[string]any, error) {
	levels, err := buildLevels(wf)
	if err != nil {
		return nil, err
	}

	for _, level := range levels {
		if len(level.Steps) == 1 {
			// Single step — execute directly without goroutine overhead
			step := wf.Steps[level.Steps[0]]
			if err := e.executeParallelStep(ctx, workflowID, step, vars); err != nil {
				return nil, err
			}
			continue
		}

		// Multiple steps — execute concurrently
		g, gCtx := errgroup.WithContext(ctx)
		for _, idx := range level.Steps {
			step := wf.Steps[idx]
			g.Go(func() error {
				return e.executeParallelStep(gCtx, workflowID, step, vars)
			})
		}
		if err := g.Wait(); err != nil {
			return nil, err
		}
	}

	return e.buildOutputs(wf, vars), nil
}

// executeParallelStep runs a single step with tracing and error conversion.
func (e *Engine) executeParallelStep(ctx context.Context, workflowID string, step parser.Step, vars *VarStore) error {
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

	if !result.success {
		if result.err != nil {
			return fmt.Errorf("step %s: %w", step.StepID, result.err)
		}
		bodyPreview := string(result.response.Body)
		if len(bodyPreview) > 500 {
			bodyPreview = bodyPreview[:500] + "..."
		}
		return fmt.Errorf("step %s: success criteria not met (status=%d, body=%s)",
			step.StepID, result.response.StatusCode, bodyPreview)
	}

	return nil
}
