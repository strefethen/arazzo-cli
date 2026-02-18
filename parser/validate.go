package parser

import (
	"fmt"
	"strings"
)

// ValidationError contains details about a validation failure.
type ValidationError struct {
	Path    string
	Message string
}

func (e ValidationError) Error() string {
	return fmt.Sprintf("%s: %s", e.Path, e.Message)
}

// Validate checks an Arazzo specification for correctness.
func Validate(spec *ArazzoSpec) error {
	var errs []string

	// Check arazzo version
	if spec.Arazzo == "" {
		errs = append(errs, "arazzo version is required")
	} else if !strings.HasPrefix(spec.Arazzo, "1.") {
		errs = append(errs, fmt.Sprintf("unsupported arazzo version: %s (expected 1.x)", spec.Arazzo))
	}

	// Check info
	if spec.Info.Title == "" {
		errs = append(errs, "info.title is required")
	}
	if spec.Info.Version == "" {
		errs = append(errs, "info.version is required")
	}

	// Check source descriptions
	sourceNames := make(map[string]bool)
	for i, src := range spec.SourceDescriptions {
		path := fmt.Sprintf("sourceDescriptions[%d]", i)
		if src.Name == "" {
			errs = append(errs, fmt.Sprintf("%s.name is required", path))
		} else if sourceNames[src.Name] {
			errs = append(errs, fmt.Sprintf("%s.name '%s' is duplicate", path, src.Name))
		} else {
			sourceNames[src.Name] = true
		}
		if src.URL == "" {
			errs = append(errs, fmt.Sprintf("%s.url is required", path))
		}
		if src.Type != "openapi" && src.Type != "arazzo" {
			errs = append(errs, fmt.Sprintf("%s.type must be 'openapi' or 'arazzo', got '%s'", path, src.Type))
		}
	}

	// Check workflows
	workflowIDs := make(map[string]bool)
	for i, wf := range spec.Workflows {
		path := fmt.Sprintf("workflows[%d]", i)
		if wf.WorkflowID == "" {
			errs = append(errs, fmt.Sprintf("%s.workflowId is required", path))
		} else if workflowIDs[wf.WorkflowID] {
			errs = append(errs, fmt.Sprintf("%s.workflowId '%s' is duplicate", path, wf.WorkflowID))
		} else {
			workflowIDs[wf.WorkflowID] = true
		}

		// Check steps
		stepIDs := make(map[string]bool)
		for j, step := range wf.Steps {
			stepPath := fmt.Sprintf("%s.steps[%d]", path, j)
			if step.StepID == "" {
				errs = append(errs, fmt.Sprintf("%s.stepId is required", stepPath))
			} else if stepIDs[step.StepID] {
				errs = append(errs, fmt.Sprintf("%s.stepId '%s' is duplicate", stepPath, step.StepID))
			} else {
				stepIDs[step.StepID] = true
			}

			// Must have operationId, operationPath, or workflowId
			hasOp := step.OperationID != "" || step.OperationPath != "" || step.WorkflowID != ""
			if !hasOp {
				errs = append(errs, fmt.Sprintf("%s must have operationId, operationPath, or workflowId", stepPath))
			}

			// Validate parameters
			for k, param := range step.Parameters {
				paramPath := fmt.Sprintf("%s.parameters[%d]", stepPath, k)
				if param.Name == "" && param.Reference == "" {
					errs = append(errs, fmt.Sprintf("%s.name is required (unless using reference)", paramPath))
				}
				if param.Value == "" && param.Reference == "" {
					errs = append(errs, fmt.Sprintf("%s must have value or reference", paramPath))
				}
				if param.In != "" && param.In != "path" && param.In != "query" && param.In != "header" && param.In != "cookie" {
					errs = append(errs, fmt.Sprintf("%s.in must be path, query, header, or cookie", paramPath))
				}
			}

			// Validate retry fields on actions
			for k, action := range step.OnFailure {
				actionPath := fmt.Sprintf("%s.onFailure[%d]", stepPath, k)
				if action.RetryAfter < 0 {
					errs = append(errs, fmt.Sprintf("%s.retryAfter must be non-negative", actionPath))
				}
				if action.RetryLimit < 0 {
					errs = append(errs, fmt.Sprintf("%s.retryLimit must be non-negative", actionPath))
				}
			}
			for k, action := range step.OnSuccess {
				actionPath := fmt.Sprintf("%s.onSuccess[%d]", stepPath, k)
				if action.RetryAfter < 0 {
					errs = append(errs, fmt.Sprintf("%s.retryAfter must be non-negative", actionPath))
				}
				if action.RetryLimit < 0 {
					errs = append(errs, fmt.Sprintf("%s.retryLimit must be non-negative", actionPath))
				}
			}
		}

		// Validate output expressions reference valid steps
		for name, expr := range wf.Outputs {
			if strings.HasPrefix(expr, "$steps.") {
				parts := strings.SplitN(strings.TrimPrefix(expr, "$steps."), ".", 2)
				if len(parts) > 0 && !stepIDs[parts[0]] {
					errs = append(errs, fmt.Sprintf("%s.outputs.%s references unknown step '%s'", path, name, parts[0]))
				}
			}
		}
	}

	if len(errs) > 0 {
		return fmt.Errorf("validation errors:\n  - %s", strings.Join(errs, "\n  - "))
	}

	return nil
}
