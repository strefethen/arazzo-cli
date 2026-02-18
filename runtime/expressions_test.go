package runtime

import (
	"net/http"
	"testing"
)

// ── Evaluate ────────────────────────────────────────────────────────────

func TestEvaluate_Literal(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	if v := eval.Evaluate("hello"); v != "hello" {
		t.Fatalf("expected 'hello', got %v", v)
	}
}

func TestEvaluate_Inputs(t *testing.T) {
	vars := NewVarStore()
	vars.SetInput("name", "Alice")
	eval := NewExpressionEvaluator(vars)

	if v := eval.Evaluate("$inputs.name"); v != "Alice" {
		t.Fatalf("expected 'Alice', got %v", v)
	}
}

func TestEvaluate_InputsMissing(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	if v := eval.Evaluate("$inputs.missing"); v != nil {
		t.Fatalf("expected nil, got %v", v)
	}
}

func TestEvaluate_StepOutputs(t *testing.T) {
	vars := NewVarStore()
	vars.SetStepOutput("s1", "token", "abc")
	eval := NewExpressionEvaluator(vars)

	if v := eval.Evaluate("$steps.s1.outputs.token"); v != "abc" {
		t.Fatalf("expected 'abc', got %v", v)
	}
}

func TestEvaluate_StepOutputsMissingStep(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	if v := eval.Evaluate("$steps.nope.outputs.x"); v != nil {
		t.Fatalf("expected nil, got %v", v)
	}
}

func TestEvaluate_StepOutputsMalformed(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	// Missing ".outputs." segment
	if v := eval.Evaluate("$steps.s1.token"); v != nil {
		t.Fatalf("expected nil for malformed step expression, got %v", v)
	}
}

func TestEvaluate_StatusCode(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 404})
	if v := eval.Evaluate("$statusCode"); v != 404 {
		t.Fatalf("expected 404, got %v", v)
	}
}

func TestEvaluate_StatusCodeNoResponse(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	if v := eval.Evaluate("$statusCode"); v != nil {
		t.Fatalf("expected nil without response, got %v", v)
	}
}

func TestEvaluate_ResponseHeader(t *testing.T) {
	resp := &Response{Headers: http.Header{"X-Foo": []string{"bar"}}}
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(resp)
	if v := eval.Evaluate("$response.header.X-Foo"); v != "bar" {
		t.Fatalf("expected 'bar', got %v", v)
	}
}

func TestEvaluate_ResponseHeaderNoResponse(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	if v := eval.Evaluate("$response.header.X-Foo"); v != nil {
		t.Fatalf("expected nil without response, got %v", v)
	}
}

func TestEvaluate_ResponseBody(t *testing.T) {
	resp := &Response{
		Body:        []byte(`{"user":{"name":"Bob"}}`),
		ContentType: "json",
	}
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(resp)
	if v := eval.Evaluate("$response.body.user.name"); v != "Bob" {
		t.Fatalf("expected 'Bob', got %v", v)
	}
}

func TestEvaluate_ResponseBodyNoResponse(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	if v := eval.Evaluate("$response.body.foo"); v != nil {
		t.Fatalf("expected nil without response, got %v", v)
	}
}

func TestEvaluate_EnvVar(t *testing.T) {
	t.Setenv("ARAZZO_TEST_EVAL", "secret")
	eval := NewExpressionEvaluator(NewVarStore())
	if v := eval.Evaluate("$env.ARAZZO_TEST_EVAL"); v != "secret" {
		t.Fatalf("expected 'secret', got %v", v)
	}
}

func TestEvaluate_UnknownExpression(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	if v := eval.Evaluate("$unknown.thing"); v != nil {
		t.Fatalf("expected nil for unknown expression, got %v", v)
	}
}

// ── EvaluateString ──────────────────────────────────────────────────────

func TestEvaluateString_Nil(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	if v := eval.EvaluateString("$inputs.missing"); v != "" {
		t.Fatalf("expected empty string for nil, got %q", v)
	}
}

func TestEvaluateString_String(t *testing.T) {
	vars := NewVarStore()
	vars.SetInput("x", "hello")
	eval := NewExpressionEvaluator(vars)
	if v := eval.EvaluateString("$inputs.x"); v != "hello" {
		t.Fatalf("expected 'hello', got %q", v)
	}
}

func TestEvaluateString_Float(t *testing.T) {
	vars := NewVarStore()
	vars.SetInput("x", 3.14)
	eval := NewExpressionEvaluator(vars)
	if v := eval.EvaluateString("$inputs.x"); v != "3.14" {
		t.Fatalf("expected '3.14', got %q", v)
	}
}

func TestEvaluateString_Int(t *testing.T) {
	vars := NewVarStore()
	vars.SetInput("x", 42)
	eval := NewExpressionEvaluator(vars)
	if v := eval.EvaluateString("$inputs.x"); v != "42" {
		t.Fatalf("expected '42', got %q", v)
	}
}

func TestEvaluateString_Int64(t *testing.T) {
	vars := NewVarStore()
	vars.SetInput("x", int64(99))
	eval := NewExpressionEvaluator(vars)
	if v := eval.EvaluateString("$inputs.x"); v != "99" {
		t.Fatalf("expected '99', got %q", v)
	}
}

func TestEvaluateString_UnsupportedType(t *testing.T) {
	vars := NewVarStore()
	vars.SetInput("x", []int{1, 2})
	eval := NewExpressionEvaluator(vars)
	if v := eval.EvaluateString("$inputs.x"); v != "" {
		t.Fatalf("expected empty string for unsupported type, got %q", v)
	}
}

// ── EvaluateCondition ───────────────────────────────────────────────────

func TestEvaluateCondition_Equality(t *testing.T) {
	resp := &Response{StatusCode: 200}
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(resp)

	if !eval.EvaluateCondition("$statusCode == 200") {
		t.Fatal("expected true for $statusCode == 200")
	}
	if eval.EvaluateCondition("$statusCode == 404") {
		t.Fatal("expected false for $statusCode == 404")
	}
}

func TestEvaluateCondition_Inequality(t *testing.T) {
	resp := &Response{StatusCode: 500}
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(resp)

	if !eval.EvaluateCondition("$statusCode != 200") {
		t.Fatal("expected true for $statusCode != 200 when status is 500")
	}
	if eval.EvaluateCondition("$statusCode != 500") {
		t.Fatal("expected false for $statusCode != 500 when status is 500")
	}
}

func TestEvaluateCondition_StringComparison(t *testing.T) {
	vars := NewVarStore()
	vars.SetStepOutput("s1", "status", "ok")
	eval := NewExpressionEvaluator(vars)

	if !eval.EvaluateCondition(`$steps.s1.outputs.status == "ok"`) {
		t.Fatal("expected true for string equality")
	}
	if eval.EvaluateCondition(`$steps.s1.outputs.status == "fail"`) {
		t.Fatal("expected false for string inequality")
	}
}

func TestEvaluateCondition_NoOperator(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	if eval.EvaluateCondition("just a string") {
		t.Fatal("expected false for condition with no operator")
	}
}

// ── parseValue ──────────────────────────────────────────────────────────

func TestParseValue_Integer(t *testing.T) {
	v := parseValue("42")
	if v != int(42) {
		t.Fatalf("expected int(42), got %v (%T)", v, v)
	}
}

func TestParseValue_Float(t *testing.T) {
	v := parseValue("3.14")
	if v != 3.14 {
		t.Fatalf("expected 3.14, got %v", v)
	}
}

func TestParseValue_BoolTrue(t *testing.T) {
	if v := parseValue("true"); v != true {
		t.Fatalf("expected true, got %v", v)
	}
}

func TestParseValue_BoolFalse(t *testing.T) {
	if v := parseValue("false"); v != false {
		t.Fatalf("expected false, got %v", v)
	}
}

func TestParseValue_QuotedString(t *testing.T) {
	if v := parseValue(`"hello"`); v != "hello" {
		t.Fatalf("expected 'hello', got %v", v)
	}
	if v := parseValue(`'world'`); v != "world" {
		t.Fatalf("expected 'world', got %v", v)
	}
}

func TestParseValue_UnquotedString(t *testing.T) {
	if v := parseValue("abc"); v != "abc" {
		t.Fatalf("expected 'abc', got %v", v)
	}
}

func TestParseValue_Whitespace(t *testing.T) {
	if v := parseValue("  200  "); v != int(200) {
		t.Fatalf("expected int(200) after trim, got %v (%T)", v, v)
	}
}

// ── compareValues ───────────────────────────────────────────────────────

func TestCompareValues_BothNil(t *testing.T) {
	if !compareValues(nil, nil) {
		t.Fatal("expected true for nil == nil")
	}
}

func TestCompareValues_OneNil(t *testing.T) {
	if compareValues(nil, 1) {
		t.Fatal("expected false for nil == 1")
	}
	if compareValues("a", nil) {
		t.Fatal("expected false for 'a' == nil")
	}
}

func TestCompareValues_NumericCrossType(t *testing.T) {
	// int vs float64
	if !compareValues(int(200), float64(200)) {
		t.Fatal("expected int(200) == float64(200)")
	}
	// int64 vs int
	if !compareValues(int64(42), int(42)) {
		t.Fatal("expected int64(42) == int(42)")
	}
}

func TestCompareValues_StringFallback(t *testing.T) {
	if !compareValues("hello", "hello") {
		t.Fatal("expected 'hello' == 'hello'")
	}
	if compareValues("hello", "world") {
		t.Fatal("expected 'hello' != 'world'")
	}
}

// ── InterpolateString ───────────────────────────────────────────────────

func TestInterpolateString_BracketSyntax(t *testing.T) {
	vars := NewVarStore()
	vars.SetInput("name", "Alice")
	eval := NewExpressionEvaluator(vars)

	result := eval.InterpolateString("Hello ${inputs.name}!")
	if result != "Hello Alice!" {
		t.Fatalf("expected 'Hello Alice!', got %q", result)
	}
}

func TestInterpolateString_DollarSyntax(t *testing.T) {
	vars := NewVarStore()
	vars.SetInput("age", 30)
	eval := NewExpressionEvaluator(vars)

	result := eval.InterpolateString("Age: $inputs.age")
	if result != "Age: 30" {
		t.Fatalf("expected 'Age: 30', got %q", result)
	}
}

func TestInterpolateString_Mixed(t *testing.T) {
	vars := NewVarStore()
	vars.SetInput("a", "X")
	vars.SetStepOutput("s1", "b", "Y")
	eval := NewExpressionEvaluator(vars)

	result := eval.InterpolateString("${inputs.a}-$steps.s1.outputs.b")
	if result != "X-Y" {
		t.Fatalf("expected 'X-Y', got %q", result)
	}
}

func TestInterpolateString_NoExpressions(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore())
	result := eval.InterpolateString("plain text")
	if result != "plain text" {
		t.Fatalf("expected 'plain text', got %q", result)
	}
}

// ── Bug regression tests ───────────────────────────────────────────────

func TestParseValue_SingleQuoteNoPanic(t *testing.T) {
	// Regression: single-char quote previously panicked due to operator
	// precedence — len(s) >= 2 didn't guard the single-quote branch.
	v := parseValue("'")
	if v != "'" {
		t.Fatalf("expected single quote literal, got %v", v)
	}
}

func TestEvaluateString_Bool(t *testing.T) {
	// Regression: booleans previously returned "" instead of "true"/"false".
	vars := NewVarStore()
	vars.SetInput("x", true)
	vars.SetInput("y", false)
	eval := NewExpressionEvaluator(vars)
	if v := eval.EvaluateString("$inputs.x"); v != "true" {
		t.Fatalf("expected 'true', got %q", v)
	}
	if v := eval.EvaluateString("$inputs.y"); v != "false" {
		t.Fatalf("expected 'false', got %q", v)
	}
}
