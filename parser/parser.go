package parser

import (
	"fmt"
	"os"

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

	if err := Validate(&spec); err != nil {
		return nil, err
	}

	return &spec, nil
}
