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
		return e.vars.GetInput(name)
	}

	// $steps.stepId.outputs.name
	if after, ok := strings.CutPrefix(rest, "steps."); ok {
		parts := strings.SplitN(after, ".outputs.", 2)
		if len(parts) == 2 {
			return e.vars.GetStepOutput(parts[0], parts[1])
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

// EvaluateCondition evaluates a condition expression with operator precedence.
// Supports: ==, !=, >, <, >=, <=, &&, ||, contains, matches, in.
func (e *ExpressionEvaluator) EvaluateCondition(condition string) bool {
	condition = strings.TrimSpace(condition)
	if condition == "" {
		return false
	}

	// Level 1: OR (lowest precedence)
	if parts := splitOutsideQuotes(condition, "||"); parts != nil {
		for _, part := range parts {
			if e.EvaluateCondition(part) {
				return true
			}
		}
		return false
	}

	// Level 2: AND
	if parts := splitOutsideQuotes(condition, "&&"); parts != nil {
		for _, part := range parts {
			if !e.EvaluateCondition(part) {
				return false
			}
		}
		return true
	}

	// Level 3: Single comparison
	return e.evaluateComparison(condition)
}

// evaluateComparison evaluates a single comparison (no && or ||).
func (e *ExpressionEvaluator) evaluateComparison(condition string) bool {
	op, idx := findOperator(condition)
	if op == "" {
		// No operator: evaluate as truthiness check
		val := resolveOperand(e, condition)
		return isTruthy(val)
	}

	left := resolveOperand(e, condition[:idx])
	right := strings.TrimSpace(condition[idx+len(op):])

	switch op {
	case "==":
		return compareValues(left, resolveOperand(e, right))
	case "!=":
		return !compareValues(left, resolveOperand(e, right))
	case ">":
		return compareOrdered(left, resolveOperand(e, right)) > 0
	case "<":
		return compareOrdered(left, resolveOperand(e, right)) < 0
	case ">=":
		return compareOrdered(left, resolveOperand(e, right)) >= 0
	case "<=":
		return compareOrdered(left, resolveOperand(e, right)) <= 0
	case " contains ":
		return strings.Contains(toString(left), toString(resolveOperand(e, right)))
	case " matches ":
		re, err := getCompiledRegex(toString(resolveOperand(e, right)))
		if err != nil {
			return false
		}
		return re.MatchString(toString(left))
	case " in ":
		return evalIn(e, left, right)
	}
	return false
}

// resolveOperand evaluates $-expressions or parses literals.
func resolveOperand(e *ExpressionEvaluator, s string) any {
	s = strings.TrimSpace(s)
	if strings.HasPrefix(s, "$") {
		return e.Evaluate(s)
	}
	return parseValue(s)
}

// compareOrdered returns -1, 0, or 1 for ordered comparison.
// Numeric values compare numerically; others compare as strings.
func compareOrdered(a, b any) int {
	aF, aOk := toFloat64(a)
	bF, bOk := toFloat64(b)
	if aOk && bOk {
		if aF < bF {
			return -1
		}
		if aF > bF {
			return 1
		}
		return 0
	}
	return strings.Compare(toString(a), toString(b))
}

// isTruthy returns true for non-nil, non-false, non-zero, non-empty values.
func isTruthy(v any) bool {
	if v == nil {
		return false
	}
	switch t := v.(type) {
	case bool:
		return t
	case int:
		return t != 0
	case int64:
		return t != 0
	case float64:
		return t != 0
	case string:
		return t != ""
	default:
		return true
	}
}

// splitOutsideQuotes splits s on delim while respecting quoted strings.
// Returns nil if delim not found outside quotes.
func splitOutsideQuotes(s, delim string) []string {
	var parts []string
	start := 0
	inQuote := byte(0)
	found := false

	for i := 0; i < len(s); i++ {
		ch := s[i]
		if inQuote != 0 {
			if ch == inQuote {
				inQuote = 0
			}
			continue
		}
		if ch == '"' || ch == '\'' {
			inQuote = ch
			continue
		}
		if i+len(delim) <= len(s) && s[i:i+len(delim)] == delim {
			parts = append(parts, s[start:i])
			start = i + len(delim)
			i += len(delim) - 1
			found = true
		}
	}
	if !found {
		return nil
	}
	return append(parts, s[start:])
}

// findOperator scans for a comparison operator outside quoted strings.
// Returns the operator and its byte offset, or ("", -1) if not found.
func findOperator(s string) (string, int) {
	inQuote := byte(0)

	// Word operators checked via separate scan
	for _, wop := range []string{" contains ", " matches ", " in "} {
		if idx := indexOutsideQuotes(s, wop); idx >= 0 {
			return wop, idx
		}
	}

	// Symbolic operators: scan left-to-right, check multi-char before single-char
	for i := 0; i < len(s); i++ {
		ch := s[i]
		if inQuote != 0 {
			if ch == inQuote {
				inQuote = 0
			}
			continue
		}
		if ch == '"' || ch == '\'' {
			inQuote = ch
			continue
		}
		if i+2 <= len(s) {
			two := s[i : i+2]
			switch two {
			case "!=", ">=", "<=", "==":
				return two, i
			}
		}
		if ch == '>' || ch == '<' {
			return string(ch), i
		}
	}
	return "", -1
}

// indexOutsideQuotes finds the first occurrence of substr in s outside quotes.
// Returns -1 if not found.
func indexOutsideQuotes(s, substr string) int {
	inQuote := byte(0)
	for i := 0; i < len(s); i++ {
		ch := s[i]
		if inQuote != 0 {
			if ch == inQuote {
				inQuote = 0
			}
			continue
		}
		if ch == '"' || ch == '\'' {
			inQuote = ch
			continue
		}
		if i+len(substr) <= len(s) && s[i:i+len(substr)] == substr {
			return i
		}
	}
	return -1
}

// evalIn checks if left matches any element in a bracket-delimited list.
func evalIn(e *ExpressionEvaluator, left any, listStr string) bool {
	listStr = strings.TrimSpace(listStr)
	if !strings.HasPrefix(listStr, "[") || !strings.HasSuffix(listStr, "]") {
		return false
	}
	inner := listStr[1 : len(listStr)-1]
	if strings.TrimSpace(inner) == "" {
		return false
	}
	for _, elem := range splitListElements(inner) {
		if compareValues(left, resolveOperand(e, elem)) {
			return true
		}
	}
	return false
}

// splitListElements splits a comma-separated list while respecting quoted strings.
func splitListElements(s string) []string {
	var elems []string
	start := 0
	inQuote := byte(0)
	for i := 0; i < len(s); i++ {
		ch := s[i]
		if inQuote != 0 {
			if ch == inQuote {
				inQuote = 0
			}
			continue
		}
		if ch == '"' || ch == '\'' {
			inQuote = ch
			continue
		}
		if ch == ',' {
			elems = append(elems, s[start:i])
			start = i + 1
		}
	}
	return append(elems, s[start:])
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
