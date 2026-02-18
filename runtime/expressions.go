package runtime

import (
	"regexp"
	"strconv"
	"strings"
)

// Package-level compiled regex pattern for performance.
var interpolateRe = regexp.MustCompile(`\$\{([^}]+)\}|\$([a-zA-Z_][a-zA-Z0-9_\.]*(?:\[[0-9]+\])*)`)

// ExpressionEvaluator evaluates Arazzo runtime expressions.
type ExpressionEvaluator struct {
	vars     *VarStore
	response *Response
}

// NewExpressionEvaluator creates an evaluator with the given variable store.
func NewExpressionEvaluator(vars *VarStore) *ExpressionEvaluator {
	return &ExpressionEvaluator{vars: vars}
}

// WithResponse sets the current response for $response expressions.
func (e *ExpressionEvaluator) WithResponse(resp *Response) *ExpressionEvaluator {
	e.response = resp
	return e
}

// Evaluate evaluates an Arazzo expression and returns its value.
// Supported expressions:
//   - $inputs.name
//   - $steps.stepId.outputs.name
//   - $response.body.path.to.field
//   - $response.body.array[0].field
//   - $statusCode
//   - Literal values (strings without $)
func (e *ExpressionEvaluator) Evaluate(expr string) any {
	if !strings.HasPrefix(expr, "$") {
		return expr // literal value
	}

	expr = strings.TrimPrefix(expr, "$")

	// $inputs.name
	if strings.HasPrefix(expr, "inputs.") {
		name := strings.TrimPrefix(expr, "inputs.")
		return e.vars.inputs[name]
	}

	// $steps.stepId.outputs.name
	if strings.HasPrefix(expr, "steps.") {
		rest := strings.TrimPrefix(expr, "steps.")
		parts := strings.SplitN(rest, ".outputs.", 2)
		if len(parts) == 2 {
			stepID := parts[0]
			name := parts[1]
			if stepOutputs, ok := e.vars.steps[stepID]; ok {
				return stepOutputs[name]
			}
		}
		return nil
	}

	// $statusCode
	if expr == "statusCode" && e.response != nil {
		return e.response.StatusCode
	}

	// $response.body.path
	if strings.HasPrefix(expr, "response.body.") && e.response != nil {
		path := strings.TrimPrefix(expr, "response.body.")
		// Convert Arazzo path to gjson path
		gjsonPath := convertToGJSONPath(path)
		return e.response.Extract(gjsonPath)
	}

	return nil
}

// EvaluateString evaluates an expression and returns it as a string.
func (e *ExpressionEvaluator) EvaluateString(expr string) string {
	val := e.Evaluate(expr)
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
		return ""
	}
}

// EvaluateCondition evaluates a simple condition expression.
// Supports: $statusCode == 200, $statusCode == 201, etc.
func (e *ExpressionEvaluator) EvaluateCondition(condition string) bool {
	// Simple pattern: $expr == value or $expr != value
	condition = strings.TrimSpace(condition)

	// Handle == comparison
	if strings.Contains(condition, "==") {
		parts := strings.SplitN(condition, "==", 2)
		if len(parts) == 2 {
			left := strings.TrimSpace(parts[0])
			right := strings.TrimSpace(parts[1])

			leftVal := e.Evaluate(left)
			rightVal := parseValue(right)

			return compareValues(leftVal, rightVal)
		}
	}

	// Handle != comparison
	if strings.Contains(condition, "!=") {
		parts := strings.SplitN(condition, "!=", 2)
		if len(parts) == 2 {
			left := strings.TrimSpace(parts[0])
			right := strings.TrimSpace(parts[1])

			leftVal := e.Evaluate(left)
			rightVal := parseValue(right)

			return !compareValues(leftVal, rightVal)
		}
	}

	return false
}

// convertToGJSONPath converts an Arazzo path to a gjson path.
// Arazzo uses periods for object access and [n] for array access.
// gjson uses the same syntax, but we need to handle some edge cases.
func convertToGJSONPath(path string) string {
	// gjson already handles most cases well
	return path
}

// parseValue parses a literal value from a condition.
func parseValue(s string) any {
	s = strings.TrimSpace(s)

	// Try integer
	if i, err := strconv.ParseInt(s, 10, 64); err == nil {
		return int(i)
	}

	// Try float
	if f, err := strconv.ParseFloat(s, 64); err == nil {
		return f
	}

	// Try boolean
	if s == "true" {
		return true
	}
	if s == "false" {
		return false
	}

	// Remove quotes for string
	if len(s) >= 2 && (s[0] == '"' && s[len(s)-1] == '"') ||
		(s[0] == '\'' && s[len(s)-1] == '\'') {
		return s[1 : len(s)-1]
	}

	return s
}

// compareValues compares two values for equality.
func compareValues(a, b any) bool {
	// Handle nil cases
	if a == nil && b == nil {
		return true
	}
	if a == nil || b == nil {
		return false
	}

	// Convert both to float64 for numeric comparison
	aFloat, aIsNum := toFloat64(a)
	bFloat, bIsNum := toFloat64(b)

	if aIsNum && bIsNum {
		return aFloat == bFloat
	}

	// String comparison
	return toString(a) == toString(b)
}

func toFloat64(v any) (float64, bool) {
	switch t := v.(type) {
	case float64:
		return t, true
	case int:
		return float64(t), true
	case int64:
		return float64(t), true
	default:
		return 0, false
	}
}

func toString(v any) string {
	switch t := v.(type) {
	case string:
		return t
	case float64:
		return strconv.FormatFloat(t, 'f', -1, 64)
	case int:
		return strconv.Itoa(t)
	case int64:
		return strconv.FormatInt(t, 10)
	default:
		return ""
	}
}

// InterpolateString replaces all ${expr} patterns in a string with their values.
func (e *ExpressionEvaluator) InterpolateString(s string) string {
	return interpolateRe.ReplaceAllStringFunc(s, func(match string) string {
		var expr string
		if strings.HasPrefix(match, "${") {
			expr = "$" + match[2:len(match)-1]
		} else {
			expr = match
		}
		return e.EvaluateString(expr)
	})
}
