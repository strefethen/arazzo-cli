package parser

import (
	"fmt"
	"os"
	"strings"

	"gopkg.in/yaml.v3"
)

// Parse loads and parses an Arazzo specification from the given file path.
func Parse(path string) (*ArazzoSpec, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("reading arazzo file: %w", err)
	}

	return ParseBytes(data)
}

// ParseBytes parses an Arazzo specification from raw YAML bytes.
func ParseBytes(data []byte) (*ArazzoSpec, error) {
	var spec ArazzoSpec
	if err := yaml.Unmarshal(data, &spec); err != nil {
		return nil, fmt.Errorf("parsing arazzo yaml: %w", err)
	}

	// Resolve $components.* references before validation
	if err := resolveComponents(&spec); err != nil {
		return nil, err
	}

	if err := Validate(&spec); err != nil {
		return nil, err
	}

	return &spec, nil
}

// resolveComponents walks all workflows/steps and replaces $components.*
// references with the actual definitions from the components object.
func resolveComponents(spec *ArazzoSpec) error {
	if spec.Components == nil {
		return nil
	}

	for i := range spec.Workflows {
		wf := &spec.Workflows[i]
		for j := range wf.Steps {
			step := &wf.Steps[j]

			// Resolve parameter references
			resolved := make([]Parameter, 0, len(step.Parameters))
			for _, param := range step.Parameters {
				if ref := param.Reference; ref != "" {
					name, ok := strings.CutPrefix(ref, "$components.parameters.")
					if !ok {
						return fmt.Errorf("step %s: unsupported parameter reference: %s", step.StepID, ref)
					}
					comp, found := spec.Components.Parameters[name]
					if !found {
						return fmt.Errorf("step %s: component parameter %q not found", step.StepID, name)
					}
					// Component provides defaults; step-level values override
					if param.Name == "" {
						param.Name = comp.Name
					}
					if param.In == "" {
						param.In = comp.In
					}
					if param.Value == "" {
						param.Value = comp.Value
					}
					param.Reference = "" // resolved
				}
				resolved = append(resolved, param)
			}
			step.Parameters = resolved

			// Resolve successAction references
			if len(step.OnSuccess) == 1 && step.OnSuccess[0].Type == "" && step.OnSuccess[0].Name != "" {
				name, ok := strings.CutPrefix(step.OnSuccess[0].Name, "$components.successActions.")
				if ok {
					actions, found := spec.Components.SuccessActions[name]
					if !found {
						return fmt.Errorf("step %s: component successAction %q not found", step.StepID, name)
					}
					step.OnSuccess = actions
				}
			}

			// Resolve failureAction references
			if len(step.OnFailure) == 1 && step.OnFailure[0].Type == "" && step.OnFailure[0].Name != "" {
				name, ok := strings.CutPrefix(step.OnFailure[0].Name, "$components.failureActions.")
				if ok {
					actions, found := spec.Components.FailureActions[name]
					if !found {
						return fmt.Errorf("step %s: component failureAction %q not found", step.StepID, name)
					}
					step.OnFailure = actions
				}
			}
		}
	}
	return nil
}
