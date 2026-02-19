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
	// With truthiness fallback, a non-empty literal string is truthy
	if !eval.EvaluateCondition("just a string") {
		t.Fatal("expected true for non-empty bare string (truthiness)")
	}
	// Empty string is falsy
	if eval.EvaluateCondition("") {
		t.Fatal("expected false for empty condition")
	}
}

// ── Rich condition operators ────────────────────────────────────────────

func TestCondition_GreaterThan(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 200})
	if !eval.EvaluateCondition("$statusCode > 199") {
		t.Fatal("expected 200 > 199")
	}
	if eval.EvaluateCondition("$statusCode > 200") {
		t.Fatal("expected 200 not > 200")
	}
	if eval.EvaluateCondition("$statusCode > 300") {
		t.Fatal("expected 200 not > 300")
	}
}

func TestCondition_LessThan(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 200})
	if !eval.EvaluateCondition("$statusCode < 300") {
		t.Fatal("expected 200 < 300")
	}
	if eval.EvaluateCondition("$statusCode < 200") {
		t.Fatal("expected 200 not < 200")
	}
	if eval.EvaluateCondition("$statusCode < 100") {
		t.Fatal("expected 200 not < 100")
	}
}

func TestCondition_GreaterEqual(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 200})
	if !eval.EvaluateCondition("$statusCode >= 200") {
		t.Fatal("expected 200 >= 200")
	}
	if !eval.EvaluateCondition("$statusCode >= 199") {
		t.Fatal("expected 200 >= 199")
	}
	if eval.EvaluateCondition("$statusCode >= 201") {
		t.Fatal("expected 200 not >= 201")
	}
}

func TestCondition_LessEqual(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 200})
	if !eval.EvaluateCondition("$statusCode <= 200") {
		t.Fatal("expected 200 <= 200")
	}
	if !eval.EvaluateCondition("$statusCode <= 300") {
		t.Fatal("expected 200 <= 300")
	}
	if eval.EvaluateCondition("$statusCode <= 199") {
		t.Fatal("expected 200 not <= 199")
	}
}

func TestCondition_And(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 200})
	if !eval.EvaluateCondition("$statusCode >= 200 && $statusCode < 300") {
		t.Fatal("expected true for 200 in [200,300)")
	}
	eval2 := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 404})
	if eval2.EvaluateCondition("$statusCode >= 200 && $statusCode < 300") {
		t.Fatal("expected false for 404 not in [200,300)")
	}
}

func TestCondition_Or(t *testing.T) {
	eval200 := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 200})
	if !eval200.EvaluateCondition("$statusCode == 200 || $statusCode == 201") {
		t.Fatal("expected true for 200")
	}
	eval201 := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 201})
	if !eval201.EvaluateCondition("$statusCode == 200 || $statusCode == 201") {
		t.Fatal("expected true for 201")
	}
	eval404 := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 404})
	if eval404.EvaluateCondition("$statusCode == 200 || $statusCode == 201") {
		t.Fatal("expected false for 404")
	}
}

func TestCondition_AndOr_Precedence(t *testing.T) {
	// a || b && c should be a || (b && c)
	// With statusCode=200: (200==200) || (200==404 && 200==500) → true || false → true
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 200})
	if !eval.EvaluateCondition("$statusCode == 200 || $statusCode == 404 && $statusCode == 500") {
		t.Fatal("expected true: OR should have lower precedence than AND")
	}
	// With statusCode=404: (404==200) || (404==404 && 404==404) → false || true → true
	eval2 := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 404})
	if !eval2.EvaluateCondition("$statusCode == 200 || $statusCode == 404 && $statusCode == 404") {
		t.Fatal("expected true: AND group should evaluate true")
	}
}

func TestCondition_Contains(t *testing.T) {
	vars := NewVarStore()
	vars.SetStepOutput("s1", "msg", "hello world")
	eval := NewExpressionEvaluator(vars)

	if !eval.EvaluateCondition(`$steps.s1.outputs.msg contains "world"`) {
		t.Fatal("expected 'hello world' contains 'world'")
	}
	if eval.EvaluateCondition(`$steps.s1.outputs.msg contains "xyz"`) {
		t.Fatal("expected 'hello world' does not contain 'xyz'")
	}
}

func TestCondition_Matches(t *testing.T) {
	vars := NewVarStore()
	vars.SetStepOutput("s1", "email", "alice@example.com")
	eval := NewExpressionEvaluator(vars)

	if !eval.EvaluateCondition(`$steps.s1.outputs.email matches "^[a-z]+@"`) {
		t.Fatal("expected email to match pattern")
	}
	if eval.EvaluateCondition(`$steps.s1.outputs.email matches "^[0-9]+"`) {
		t.Fatal("expected email not to match digits pattern")
	}
}

func TestCondition_Matches_InvalidRegex(t *testing.T) {
	vars := NewVarStore()
	vars.SetStepOutput("s1", "val", "test")
	eval := NewExpressionEvaluator(vars)

	// Invalid regex should return false, not panic
	if eval.EvaluateCondition(`$steps.s1.outputs.val matches "[invalid"`) {
		t.Fatal("expected false for invalid regex")
	}
}

func TestCondition_In(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 201})
	if !eval.EvaluateCondition("$statusCode in [200, 201, 204]") {
		t.Fatal("expected 201 in [200, 201, 204]")
	}
	eval2 := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 404})
	if eval2.EvaluateCondition("$statusCode in [200, 201, 204]") {
		t.Fatal("expected 404 not in [200, 201, 204]")
	}
}

func TestCondition_In_Strings(t *testing.T) {
	vars := NewVarStore()
	vars.SetStepOutput("s1", "role", "admin")
	eval := NewExpressionEvaluator(vars)

	if !eval.EvaluateCondition(`$steps.s1.outputs.role in ["admin", "superadmin"]`) {
		t.Fatal("expected 'admin' in list")
	}
	vars2 := NewVarStore()
	vars2.SetStepOutput("s1", "role", "viewer")
	eval2 := NewExpressionEvaluator(vars2)
	if eval2.EvaluateCondition(`$steps.s1.outputs.role in ["admin", "superadmin"]`) {
		t.Fatal("expected 'viewer' not in list")
	}
}

func TestCondition_In_CommaInString(t *testing.T) {
	vars := NewVarStore()
	vars.SetStepOutput("s1", "val", "hello, world")
	eval := NewExpressionEvaluator(vars)

	if !eval.EvaluateCondition(`$steps.s1.outputs.val in ["hello, world", "foo"]`) {
		t.Fatal("expected match with comma inside quoted string")
	}
}

func TestCondition_In_Empty(t *testing.T) {
	eval := NewExpressionEvaluator(NewVarStore()).WithResponse(&Response{StatusCode: 200})
	if eval.EvaluateCondition("$statusCode in []") {
		t.Fatal("expected false for empty list")
	}
}

func TestCondition_ExpressionBothSides(t *testing.T) {
	vars := NewVarStore()
	vars.SetInput("expected", 200)
	eval := NewExpressionEvaluator(vars).WithResponse(&Response{StatusCode: 200})

	if !eval.EvaluateCondition("$statusCode == $inputs.expected") {
		t.Fatal("expected true for expression on both sides")
	}
	vars2 := NewVarStore()
	vars2.SetInput("expected", 404)
	eval2 := NewExpressionEvaluator(vars2).WithResponse(&Response{StatusCode: 200})
	if eval2.EvaluateCondition("$statusCode == $inputs.expected") {
		t.Fatal("expected false for mismatched expression values")
	}
}

func TestCondition_StringComparison_Ordered(t *testing.T) {
	vars := NewVarStore()
	vars.SetStepOutput("s1", "val", "b")
	eval := NewExpressionEvaluator(vars)

	if !eval.EvaluateCondition(`$steps.s1.outputs.val > "a"`) {
		t.Fatal("expected 'b' > 'a'")
	}
	if eval.EvaluateCondition(`$steps.s1.outputs.val > "c"`) {
		t.Fatal("expected 'b' not > 'c'")
	}
}

func TestCondition_Truthiness(t *testing.T) {
	vars := NewVarStore()
	vars.SetInput("flag", true)
	vars.SetInput("zero", 0)
	vars.SetInput("empty", "")
	eval := NewExpressionEvaluator(vars)

	if !eval.EvaluateCondition("$inputs.flag") {
		t.Fatal("expected true for truthy bool")
	}
	if eval.EvaluateCondition("$inputs.zero") {
		t.Fatal("expected false for zero")
	}
	if eval.EvaluateCondition("$inputs.empty") {
		t.Fatal("expected false for empty string")
	}
	if eval.EvaluateCondition("$inputs.missing") {
		t.Fatal("expected false for nil")
	}
}

func TestCondition_OperatorInQuotedString(t *testing.T) {
	vars := NewVarStore()
	vars.SetStepOutput("s1", "msg", "status >= ok")
	eval := NewExpressionEvaluator(vars)

	if !eval.EvaluateCondition(`$steps.s1.outputs.msg == "status >= ok"`) {
		t.Fatal("expected match — operator inside quotes should not be parsed")
	}
}

func TestCompareOrdered(t *testing.T) {
	// Numeric
	if compareOrdered(100, 200) >= 0 {
		t.Fatal("expected 100 < 200")
	}
	if compareOrdered(200, 200) != 0 {
		t.Fatal("expected 200 == 200")
	}
	if compareOrdered(300, 200) <= 0 {
		t.Fatal("expected 300 > 200")
	}
	// String
	if compareOrdered("apple", "banana") >= 0 {
		t.Fatal("expected apple < banana")
	}
	// Cross-type numeric
	if compareOrdered(int(10), float64(10)) != 0 {
		t.Fatal("expected int(10) == float64(10)")
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
