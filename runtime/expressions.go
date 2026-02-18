package runtime

import (
	"os"
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
	rest, ok := strings.CutPrefix(expr, "$")
	if !ok {
		return expr // literal value
	}

	// $env.VAR_NAME
	if name, ok := strings.CutPrefix(rest, "env."); ok {
		return os.Getenv(name)
	}

	// $inputs.name
	if name, ok := strings.CutPrefix(rest, "inputs."); ok {
		return e.vars.inputs[name]
	}

	// $steps.stepId.outputs.name
	if after, ok := strings.CutPrefix(rest, "steps."); ok {
		parts := strings.SplitN(after, ".outputs.", 2)
		if len(parts) == 2 {
			if stepOutputs, ok := e.vars.steps[parts[0]]; ok {
				return stepOutputs[parts[1]]
			}
		}
		return nil
	}

	// $statusCode
	if rest == "statusCode" && e.response != nil {
		return e.response.StatusCode
	}

	// $response.header.Name
	if name, ok := strings.CutPrefix(rest, "response.header."); ok && e.response != nil {
		return e.response.Headers.Get(name)
	}

	// $response.body.path
	if path, ok := strings.CutPrefix(rest, "response.body."); ok && e.response != nil {
		return e.response.Extract(path)
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
	case bool:
		if t {
			return "true"
		}
		return "false"
	default:
		return ""
	}
}

// EvaluateCondition evaluates a simple condition expression.
// Supports: $statusCode == 200, $statusCode == 201, etc.
func (e *ExpressionEvaluator) EvaluateCondition(condition string) bool {
	condition = strings.TrimSpace(condition)

	// Check for != first (must come before == check since "!=" contains no "==")
	if idx := strings.Index(condition, "!="); idx >= 0 {
		left := strings.TrimSpace(condition[:idx])
		right := strings.TrimSpace(condition[idx+2:])
		return !compareValues(e.Evaluate(left), parseValue(right))
	}

	// Check for == comparison
	if idx := strings.Index(condition, "=="); idx >= 0 {
		left := strings.TrimSpace(condition[:idx])
		right := strings.TrimSpace(condition[idx+2:])
		return compareValues(e.Evaluate(left), parseValue(right))
	}

	return false
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
	if len(s) >= 2 && ((s[0] == '"' && s[len(s)-1] == '"') ||
		(s[0] == '\'' && s[len(s)-1] == '\'')) {
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
