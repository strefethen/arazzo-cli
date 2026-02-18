// Package parser provides types and parsing for Arazzo 1.0 specifications.
package parser

// ArazzoSpec represents the root of an Arazzo specification.
type ArazzoSpec struct {
	Arazzo             string              `yaml:"arazzo"`
	Info               Info                `yaml:"info"`
	SourceDescriptions []SourceDescription `yaml:"sourceDescriptions"`
	Workflows          []Workflow          `yaml:"workflows"`
	Components         *Components         `yaml:"components,omitempty"`
}

// Components holds reusable definitions that can be referenced via $components.
type Components struct {
	Inputs         map[string]*SchemaObject `yaml:"inputs,omitempty"`
	Parameters     map[string]Parameter     `yaml:"parameters,omitempty"`
	SuccessActions map[string][]OnAction    `yaml:"successActions,omitempty"`
	FailureActions map[string][]OnAction    `yaml:"failureActions,omitempty"`
}

// Info contains metadata about the specification.
type Info struct {
	Title       string `yaml:"title"`
	Version     string `yaml:"version"`
	Description string `yaml:"description,omitempty"`
}

// SourceDescription defines an API source used by workflows.
type SourceDescription struct {
	Name string `yaml:"name"`
	URL  string `yaml:"url"`
	Type string `yaml:"type"` // "openapi" or "arazzo"
}

// Workflow represents a single workflow in the specification.
type Workflow struct {
	WorkflowID  string            `yaml:"workflowId"`
	Summary     string            `yaml:"summary,omitempty"`
	Description string            `yaml:"description,omitempty"`
	Inputs      *SchemaObject     `yaml:"inputs,omitempty"`
	Steps       []Step            `yaml:"steps"`
	Outputs     map[string]string `yaml:"outputs,omitempty"`
}

// SchemaObject represents a JSON Schema-like object for inputs/outputs.
type SchemaObject struct {
	Type       string                 `yaml:"type"`
	Properties map[string]PropertyDef `yaml:"properties,omitempty"`
	Required   []string               `yaml:"required,omitempty"`
}

// PropertyDef defines a single property in a schema.
type PropertyDef struct {
	Type        string `yaml:"type"`
	Description string `yaml:"description,omitempty"`
	Format      string `yaml:"format,omitempty"`
	Default     any    `yaml:"default,omitempty"`
}

// Step represents a single step in a workflow.
type Step struct {
	StepID          string             `yaml:"stepId"`
	OperationID     string             `yaml:"operationId,omitempty"`
	OperationPath   string             `yaml:"operationPath,omitempty"`
	WorkflowID      string             `yaml:"workflowId,omitempty"`
	Parameters      []Parameter        `yaml:"parameters,omitempty"`
	RequestBody     *RequestBody       `yaml:"requestBody,omitempty"`
	SuccessCriteria []SuccessCriterion `yaml:"successCriteria,omitempty"`
	OnSuccess       []OnAction         `yaml:"onSuccess,omitempty"`
	OnFailure       []OnAction         `yaml:"onFailure,omitempty"`
	Outputs         map[string]string  `yaml:"outputs,omitempty"`
}

// Parameter represents a parameter to pass to an operation.
type Parameter struct {
	Name      string `yaml:"name"`
	In        string `yaml:"in"` // "path", "query", "header", "cookie"
	Value     string `yaml:"value"`
	Reference string `yaml:"reference,omitempty"`
}

// RequestBody represents a request body for an operation.
type RequestBody struct {
	ContentType string `yaml:"contentType,omitempty"`
	Payload     any    `yaml:"payload,omitempty"`
	Reference   string `yaml:"reference,omitempty"`
}

// SuccessCriterion defines a condition for step success.
type SuccessCriterion struct {
	Condition string `yaml:"condition"`
	Context   string `yaml:"context,omitempty"`
	Type      string `yaml:"type,omitempty"` // "simple", "regex", "jsonpath", "xpath"
}

// OnAction defines an action to take on success or failure.
type OnAction struct {
	Name       string             `yaml:"name,omitempty"`
	Type       string             `yaml:"type"` // "goto", "end", "retry"
	WorkflowID string             `yaml:"workflowId,omitempty"`
	StepID     string             `yaml:"stepId,omitempty"`
	RetryAfter int                `yaml:"retryAfter,omitempty"` // seconds before retry
	RetryLimit int                `yaml:"retryLimit,omitempty"` // max retry attempts (0 = default)
	Criteria   []SuccessCriterion `yaml:"criteria,omitempty"`
}
