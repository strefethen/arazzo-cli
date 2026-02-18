package runtime

import (
	"fmt"
	"strconv"
	"strings"
)

// VarStore holds workflow variables including inputs and step outputs.
type VarStore struct {
	inputs map[string]any
	steps  map[string]map[string]any // stepId -> outputName -> value
}

// NewVarStore creates an empty VarStore.
func NewVarStore() *VarStore {
	return &VarStore{
		inputs: make(map[string]any),
		steps:  make(map[string]map[string]any),
	}
}

// SetInput sets an input variable.
func (v *VarStore) SetInput(name string, value any) {
	v.inputs[name] = value
}

// SetStepOutput sets an output for a step.
func (v *VarStore) SetStepOutput(stepID, name string, value any) {
	if v.steps[stepID] == nil {
		v.steps[stepID] = make(map[string]any)
	}
	v.steps[stepID][name] = value
}

// Get retrieves a value by expression path.
// Supported expressions:
//   - $inputs.name
//   - $steps.stepId.outputs.name
func (v *VarStore) Get(expr string) any {
	expr = strings.TrimPrefix(expr, "$")

	if strings.HasPrefix(expr, "inputs.") {
		name := strings.TrimPrefix(expr, "inputs.")
		return v.inputs[name]
	}

	if strings.HasPrefix(expr, "steps.") {
		// Format: steps.stepId.outputs.name
		rest := strings.TrimPrefix(expr, "steps.")
		parts := strings.SplitN(rest, ".outputs.", 2)
		if len(parts) != 2 {
			return nil
		}
		stepID := parts[0]
		name := parts[1]
		if stepOutputs, ok := v.steps[stepID]; ok {
			return stepOutputs[name]
		}
		return nil
	}

	return nil
}

// GetString retrieves a string value by expression path.
func (v *VarStore) GetString(expr string) string {
	val := v.Get(expr)
	if val == nil {
		return ""
	}
	switch t := val.(type) {
	case string:
		return t
	case float64:
		return strconv.FormatFloat(t, 'f', -1, 64)
	case int:
		return strconv.Itoa(t)
	case int64:
		return strconv.FormatInt(t, 10)
	default:
		return fmt.Sprintf("%v", val)
	}
}

// GetFloat retrieves a float64 value by expression path.
func (v *VarStore) GetFloat(expr string) float64 {
	val := v.Get(expr)
	if val == nil {
		return 0
	}
	switch t := val.(type) {
	case float64:
		return t
	case int:
		return float64(t)
	case int64:
		return float64(t)
	case string:
		f, _ := strconv.ParseFloat(t, 64)
		return f
	default:
		return 0
	}
}

// GetInt retrieves an int64 value by expression path.
func (v *VarStore) GetInt(expr string) int64 {
	val := v.Get(expr)
	if val == nil {
		return 0
	}
	switch t := val.(type) {
	case int64:
		return t
	case int:
		return int64(t)
	case float64:
		return int64(t)
	case string:
		i, _ := strconv.ParseInt(t, 10, 64)
		return i
	default:
		return 0
	}
}

// GetBool retrieves a bool value by expression path.
func (v *VarStore) GetBool(expr string) bool {
	val := v.Get(expr)
	if val == nil {
		return false
	}
	switch t := val.(type) {
	case bool:
		return t
	case string:
		return t == "true" || t == "1"
	case int:
		return t != 0
	case int64:
		return t != 0
	case float64:
		return t != 0
	default:
		return false
	}
}

// GetInputs returns all input values.
func (v *VarStore) GetInputs() map[string]any {
	return v.inputs
}

// GetStepOutputs returns all outputs for a step.
func (v *VarStore) GetStepOutputs(stepID string) map[string]any {
	return v.steps[stepID]
}
