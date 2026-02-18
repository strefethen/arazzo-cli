package runtime

import "testing"

// ── Get ────────────────────────────────────────────────────────────────

func TestGet_Input(t *testing.T) {
	v := NewVarStore()
	v.SetInput("name", "Alice")
	if got := v.Get("$inputs.name"); got != "Alice" {
		t.Fatalf("expected 'Alice', got %v", got)
	}
}

func TestGet_StepOutput(t *testing.T) {
	v := NewVarStore()
	v.SetStepOutput("s1", "token", "abc")
	if got := v.Get("$steps.s1.outputs.token"); got != "abc" {
		t.Fatalf("expected 'abc', got %v", got)
	}
}

func TestGet_StepOutputMalformed(t *testing.T) {
	v := NewVarStore()
	v.SetStepOutput("s1", "token", "abc")
	if got := v.Get("$steps.s1.token"); got != nil {
		t.Fatalf("expected nil for malformed step expr, got %v", got)
	}
}

func TestGet_MissingStep(t *testing.T) {
	v := NewVarStore()
	if got := v.Get("$steps.nope.outputs.x"); got != nil {
		t.Fatalf("expected nil for missing step, got %v", got)
	}
}

func TestGet_UnknownPrefix(t *testing.T) {
	v := NewVarStore()
	if got := v.Get("$foo.bar"); got != nil {
		t.Fatalf("expected nil for unknown prefix, got %v", got)
	}
}

// ── GetString ──────────────────────────────────────────────────────────

func TestGetString_Nil(t *testing.T) {
	v := NewVarStore()
	if got := v.GetString("$inputs.missing"); got != "" {
		t.Fatalf("expected empty string, got %q", got)
	}
}

func TestGetString_String(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", "hello")
	if got := v.GetString("$inputs.x"); got != "hello" {
		t.Fatalf("expected 'hello', got %q", got)
	}
}

func TestGetString_Float(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", float64(3.14))
	if got := v.GetString("$inputs.x"); got != "3.14" {
		t.Fatalf("expected '3.14', got %q", got)
	}
}

func TestGetString_Int(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", 42)
	if got := v.GetString("$inputs.x"); got != "42" {
		t.Fatalf("expected '42', got %q", got)
	}
}

func TestGetString_Int64(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", int64(99))
	if got := v.GetString("$inputs.x"); got != "99" {
		t.Fatalf("expected '99', got %q", got)
	}
}

func TestGetString_Default(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", []int{1, 2})
	got := v.GetString("$inputs.x")
	if got != "[1 2]" {
		t.Fatalf("expected '[1 2]', got %q", got)
	}
}

// ── GetFloat ───────────────────────────────────────────────────────────

func TestGetFloat_Nil(t *testing.T) {
	v := NewVarStore()
	if got := v.GetFloat("$inputs.missing"); got != 0 {
		t.Fatalf("expected 0, got %v", got)
	}
}

func TestGetFloat_Float(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", float64(3.14))
	if got := v.GetFloat("$inputs.x"); got != 3.14 {
		t.Fatalf("expected 3.14, got %v", got)
	}
}

func TestGetFloat_Int(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", 42)
	if got := v.GetFloat("$inputs.x"); got != 42.0 {
		t.Fatalf("expected 42.0, got %v", got)
	}
}

func TestGetFloat_Int64(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", int64(99))
	if got := v.GetFloat("$inputs.x"); got != 99.0 {
		t.Fatalf("expected 99.0, got %v", got)
	}
}

func TestGetFloat_String(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", "3.14")
	if got := v.GetFloat("$inputs.x"); got != 3.14 {
		t.Fatalf("expected 3.14, got %v", got)
	}
}

func TestGetFloat_Default(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", true)
	if got := v.GetFloat("$inputs.x"); got != 0 {
		t.Fatalf("expected 0 for unsupported type, got %v", got)
	}
}

// ── GetInt ──────────────────────────────────────────────────────────────

func TestGetInt_Nil(t *testing.T) {
	v := NewVarStore()
	if got := v.GetInt("$inputs.missing"); got != 0 {
		t.Fatalf("expected 0, got %v", got)
	}
}

func TestGetInt_Int64(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", int64(99))
	if got := v.GetInt("$inputs.x"); got != 99 {
		t.Fatalf("expected 99, got %v", got)
	}
}

func TestGetInt_Int(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", 42)
	if got := v.GetInt("$inputs.x"); got != 42 {
		t.Fatalf("expected 42, got %v", got)
	}
}

func TestGetInt_Float(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", float64(3.9))
	if got := v.GetInt("$inputs.x"); got != 3 {
		t.Fatalf("expected 3 (truncated), got %v", got)
	}
}

func TestGetInt_String(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", "42")
	if got := v.GetInt("$inputs.x"); got != 42 {
		t.Fatalf("expected 42, got %v", got)
	}
}

func TestGetInt_Default(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", true)
	if got := v.GetInt("$inputs.x"); got != 0 {
		t.Fatalf("expected 0 for unsupported type, got %v", got)
	}
}

// ── GetBool ─────────────────────────────────────────────────────────────

func TestGetBool_Nil(t *testing.T) {
	v := NewVarStore()
	if v.GetBool("$inputs.missing") {
		t.Fatal("expected false for missing key")
	}
}

func TestGetBool_True(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", true)
	if !v.GetBool("$inputs.x") {
		t.Fatal("expected true")
	}
}

func TestGetBool_False(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", false)
	if v.GetBool("$inputs.x") {
		t.Fatal("expected false")
	}
}

func TestGetBool_StringTrue(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", "true")
	if !v.GetBool("$inputs.x") {
		t.Fatal("expected true for 'true'")
	}
	v.SetInput("x", "1")
	if !v.GetBool("$inputs.x") {
		t.Fatal("expected true for '1'")
	}
}

func TestGetBool_StringFalse(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", "false")
	if v.GetBool("$inputs.x") {
		t.Fatal("expected false for 'false'")
	}
}

func TestGetBool_IntNonZero(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", 1)
	if !v.GetBool("$inputs.x") {
		t.Fatal("expected true for int(1)")
	}
	v.SetInput("x", 0)
	if v.GetBool("$inputs.x") {
		t.Fatal("expected false for int(0)")
	}
}

func TestGetBool_Int64(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", int64(1))
	if !v.GetBool("$inputs.x") {
		t.Fatal("expected true for int64(1)")
	}
	v.SetInput("x", int64(0))
	if v.GetBool("$inputs.x") {
		t.Fatal("expected false for int64(0)")
	}
}

func TestGetBool_Float(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", float64(1.0))
	if !v.GetBool("$inputs.x") {
		t.Fatal("expected true for float64(1.0)")
	}
	v.SetInput("x", float64(0))
	if v.GetBool("$inputs.x") {
		t.Fatal("expected false for float64(0)")
	}
}

func TestGetBool_Default(t *testing.T) {
	v := NewVarStore()
	v.SetInput("x", []int{1, 2})
	if v.GetBool("$inputs.x") {
		t.Fatal("expected false for unsupported type")
	}
}

// ── GetInputs / GetStepOutputs ─────────────────────────────────────────

func TestGetInputs(t *testing.T) {
	v := NewVarStore()
	v.SetInput("a", 1)
	v.SetInput("b", "two")
	inputs := v.GetInputs()
	if len(inputs) != 2 {
		t.Fatalf("expected 2 inputs, got %d", len(inputs))
	}
	if inputs["a"] != 1 {
		t.Fatalf("expected a=1, got %v", inputs["a"])
	}
}

func TestGetStepOutputs(t *testing.T) {
	v := NewVarStore()
	v.SetStepOutput("s1", "x", "val")
	outputs := v.GetStepOutputs("s1")
	if outputs["x"] != "val" {
		t.Fatalf("expected x='val', got %v", outputs["x"])
	}
	if v.GetStepOutputs("unknown") != nil {
		t.Fatal("expected nil for unknown step")
	}
}
