// Package main provides the arazzo CLI for executing Arazzo 1.0 workflows.
package main

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/spf13/cobra"
	"github.com/strefethen/arazzo-cli/parser"
	"github.com/strefethen/arazzo-cli/runtime"
)

func main() {
	loadEnvFile(".env")

	if err := rootCmd.Execute(); err != nil {
		os.Exit(1)
	}
}

// Version is set at build time or defaults to dev.
var version = "0.1.0"

var rootCmd = &cobra.Command{
	Use:     "arazzo",
	Short:   "Execute Arazzo 1.0 workflows",
	Version: version,
	Long: `arazzo is a CLI tool for executing Arazzo 1.0 workflow specifications
without code generation. Designed for both human and agent usage.

All commands support --json for structured machine-readable output.`,
}

// ── run ─────────────────────────────────────────────────────────────────

var runCmd = &cobra.Command{
	Use:   "run <spec.arazzo.yaml> <workflow-id>",
	Short: "Execute a workflow from a spec file",
	Args:  cobra.ExactArgs(2),
	RunE:  runWorkflow,
}

// ── validate ────────────────────────────────────────────────────────────

var validateCmd = &cobra.Command{
	Use:   "validate <spec.arazzo.yaml>",
	Short: "Validate an Arazzo spec file",
	Args:  cobra.ExactArgs(1),
	RunE:  validateSpec,
}

// ── list ────────────────────────────────────────────────────────────────

var listCmd = &cobra.Command{
	Use:   "list <spec.arazzo.yaml>",
	Short: "List workflows in a spec file",
	Args:  cobra.ExactArgs(1),
	RunE:  listWorkflows,
}

// ── catalog ─────────────────────────────────────────────────────────────

var catalogCmd = &cobra.Command{
	Use:   "catalog <dir>",
	Short: "List all workflows across all specs in a directory",
	Args:  cobra.ExactArgs(1),
	RunE:  catalogWorkflows,
}

// ── show ────────────────────────────────────────────────────────────────

var showCmd = &cobra.Command{
	Use:   "show <workflow-id>",
	Short: "Show details about a workflow including inputs and outputs",
	Args:  cobra.ExactArgs(1),
	RunE:  showWorkflow,
}

// ── flags ───────────────────────────────────────────────────────────────

var (
	inputFlags   []string
	timeoutFlag  time.Duration
	verboseFlag  bool
	jsonFlag     bool
	parallelFlag bool
	dryRunFlag   bool
	providersDir string
	headerFlags  []string
)

func init() {
	rootCmd.AddCommand(runCmd, validateCmd, listCmd, catalogCmd, showCmd)

	// Global flags
	rootCmd.PersistentFlags().BoolVar(&jsonFlag, "json", false, "Output as JSON (for agent consumption)")

	// run
	runCmd.Flags().StringArrayVarP(&inputFlags, "input", "i", nil, "Input values as key=value pairs")
	runCmd.Flags().DurationVarP(&timeoutFlag, "timeout", "t", 30*time.Second, "Request timeout")
	runCmd.Flags().BoolVarP(&verboseFlag, "verbose", "v", false, "Verbose output")
	runCmd.Flags().StringArrayVarP(&headerFlags, "header", "H", nil, "HTTP headers as key=value pairs")
	runCmd.Flags().BoolVar(&parallelFlag, "parallel", false, "Execute independent steps concurrently")
	runCmd.Flags().BoolVar(&dryRunFlag, "dry-run", false, "Resolve expressions and print requests without sending")

	// show
	showCmd.Flags().StringVar(&providersDir, "dir", ".", "Directory to search for workflow specs")

	// catalog inherits --json from root
}

// ── JSON output types ───────────────────────────────────────────────────

type catalogEntry struct {
	File        string         `json:"file"`
	Title       string         `json:"title"`
	Description string         `json:"description,omitempty"`
	Version     string         `json:"version"`
	Sources     []sourceInfo   `json:"sources"`
	Workflows   []workflowInfo `json:"workflows"`
}

type sourceInfo struct {
	Name string `json:"name"`
	URL  string `json:"url"`
	Type string `json:"type"`
}

type workflowInfo struct {
	ID      string   `json:"id"`
	Summary string   `json:"summary,omitempty"`
	Inputs  []string `json:"inputs"`
	Outputs []string `json:"outputs"`
}

type workflowDetail struct {
	ID      string                 `json:"id"`
	File    string                 `json:"file"`
	Title   string                 `json:"title"`
	Summary string                 `json:"summary,omitempty"`
	Steps   int                    `json:"steps"`
	Inputs  map[string]inputDetail `json:"inputs"`
	Outputs []string               `json:"outputs"`
	Sources []sourceInfo           `json:"sources"`
}

type inputDetail struct {
	Type        string `json:"type"`
	Required    bool   `json:"required"`
	Description string `json:"description,omitempty"`
}

type validateResult struct {
	Valid     bool     `json:"valid"`
	File      string   `json:"file"`
	Version   string   `json:"version,omitempty"`
	Title     string   `json:"title,omitempty"`
	Workflows int      `json:"workflows,omitempty"`
	Sources   int      `json:"sources,omitempty"`
	Errors    []string `json:"errors,omitempty"`
}

// ── run command ─────────────────────────────────────────────────────────

func runWorkflow(cmd *cobra.Command, args []string) error {
	specPath := args[0]
	workflowID := args[1]

	spec, err := parser.Parse(specPath)
	if err != nil {
		if jsonFlag {
			return outputJSON(map[string]any{"error": err.Error()})
		}
		return fmt.Errorf("parsing spec: %w", err)
	}

	inputs := make(map[string]any)
	for _, input := range inputFlags {
		parts := strings.SplitN(input, "=", 2)
		if len(parts) != 2 {
			return fmt.Errorf("invalid input format: %q (expected key=value)", input)
		}
		inputs[parts[0]] = parseInputValue(parts[1])
	}

	if verboseFlag {
		fmt.Fprintf(os.Stderr, "Executing workflow: %s\n", workflowID)
		fmt.Fprintf(os.Stderr, "Inputs: %v\n", inputs)
	}

	// Build client options
	opts := []runtime.ClientOption{runtime.WithTimeout(timeoutFlag)}
	for _, h := range headerFlags {
		parts := strings.SplitN(h, "=", 2)
		if len(parts) == 2 {
			opts = append(opts, runtime.WithHeader(parts[0], parts[1]))
		}
	}

	engine := runtime.NewEngine(spec, opts...)
	engine.SetParallelMode(parallelFlag)
	engine.SetDryRunMode(dryRunFlag)

	ctx, cancel := context.WithTimeout(context.Background(), timeoutFlag*10)
	defer cancel()

	outputs, err := engine.Execute(ctx, workflowID, inputs)
	if err != nil {
		if jsonFlag {
			return outputJSON(map[string]any{"error": err.Error()})
		}
		return err
	}

	if dryRunFlag {
		reqs := engine.DryRunRequests()
		if jsonFlag {
			return outputJSON(reqs)
		}
		for _, r := range reqs {
			fmt.Printf("%s %s\n", r.Method, r.URL)
			for k, v := range r.Headers {
				fmt.Printf("  %s: %s\n", k, v)
			}
			if r.Body != nil {
				fmt.Printf("  Body: %s\n", string(r.Body))
			}
			fmt.Println()
		}
		return nil
	}

	return outputJSON(outputs)
}

// ── validate command ────────────────────────────────────────────────────

func validateSpec(cmd *cobra.Command, args []string) error {
	specPath := args[0]

	spec, err := parser.Parse(specPath)
	if err != nil {
		if jsonFlag {
			return outputJSON(validateResult{
				Valid:  false,
				File:   specPath,
				Errors: []string{err.Error()},
			})
		}
		return fmt.Errorf("validation failed: %w", err)
	}

	if jsonFlag {
		return outputJSON(validateResult{
			Valid:     true,
			File:      specPath,
			Version:   spec.Arazzo,
			Title:     spec.Info.Title,
			Workflows: len(spec.Workflows),
			Sources:   len(spec.SourceDescriptions),
		})
	}

	fmt.Printf("Valid Arazzo %s spec: %s\n", spec.Arazzo, spec.Info.Title)
	fmt.Printf("  Version: %s\n", spec.Info.Version)
	fmt.Printf("  Workflows: %d\n", len(spec.Workflows))
	fmt.Printf("  Sources: %d\n", len(spec.SourceDescriptions))
	return nil
}

// ── list command ────────────────────────────────────────────────────────

func listWorkflows(cmd *cobra.Command, args []string) error {
	specPath := args[0]

	spec, err := parser.Parse(specPath)
	if err != nil {
		return err
	}

	if jsonFlag {
		var workflows []workflowInfo
		for _, wf := range spec.Workflows {
			workflows = append(workflows, buildWorkflowInfo(&wf))
		}
		return outputJSON(workflows)
	}

	fmt.Printf("Workflows in %s:\n\n", spec.Info.Title)
	for _, wf := range spec.Workflows {
		fmt.Printf("  %s\n", wf.WorkflowID)
		if wf.Summary != "" {
			fmt.Printf("    Summary: %s\n", wf.Summary)
		}
		if wf.Inputs != nil && len(wf.Inputs.Properties) > 0 {
			fmt.Printf("    Inputs:\n")
			for name, prop := range wf.Inputs.Properties {
				required := ""
				for _, r := range wf.Inputs.Required {
					if r == name {
						required = " (required)"
						break
					}
				}
				fmt.Printf("      - %s: %s%s\n", name, prop.Type, required)
			}
		}
		if len(wf.Outputs) > 0 {
			fmt.Printf("    Outputs: %v\n", mapKeys(wf.Outputs))
		}
		fmt.Println()
	}

	return nil
}

// ── catalog command ─────────────────────────────────────────────────────

func catalogWorkflows(cmd *cobra.Command, args []string) error {
	dir := args[0]

	entries, err := os.ReadDir(dir)
	if err != nil {
		return fmt.Errorf("reading directory %q: %w", dir, err)
	}

	var catalog []catalogEntry

	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".yaml") {
			continue
		}
		specPath := filepath.Join(dir, entry.Name())
		spec, err := parser.Parse(specPath)
		if err != nil {
			if verboseFlag {
				fmt.Fprintf(os.Stderr, "skipping %s: %v\n", entry.Name(), err)
			}
			continue
		}

		ce := catalogEntry{
			File:        entry.Name(),
			Title:       spec.Info.Title,
			Description: spec.Info.Description,
			Version:     spec.Info.Version,
			Sources:     buildSources(spec),
			Workflows:   []workflowInfo{},
		}

		for _, wf := range spec.Workflows {
			ce.Workflows = append(ce.Workflows, buildWorkflowInfo(&wf))
		}

		catalog = append(catalog, ce)
	}

	if jsonFlag {
		return outputJSON(catalog)
	}

	// Text table output
	type row struct {
		file       string
		workflowID string
		summary    string
	}
	var rows []row
	maxFile, maxWF := 4, 11
	for _, ce := range catalog {
		for _, wf := range ce.Workflows {
			r := row{file: ce.File, workflowID: wf.ID, summary: wf.Summary}
			rows = append(rows, r)
			if len(r.file) > maxFile {
				maxFile = len(r.file)
			}
			if len(r.workflowID) > maxWF {
				maxWF = len(r.workflowID)
			}
		}
	}

	fmtStr := fmt.Sprintf("%%-%ds  %%-%ds  %%s\n", maxFile, maxWF)
	fmt.Printf(fmtStr, "File", "Workflow ID", "Summary")
	fmt.Printf(fmtStr, strings.Repeat("-", maxFile), strings.Repeat("-", maxWF), strings.Repeat("-", 40))
	for _, r := range rows {
		fmt.Printf(fmtStr, r.file, r.workflowID, r.summary)
	}

	return nil
}

// ── show command ────────────────────────────────────────────────────────

func showWorkflow(cmd *cobra.Command, args []string) error {
	workflowID := args[0]

	spec, filename, err := findWorkflow(providersDir, workflowID)
	if err != nil {
		if jsonFlag {
			return outputJSON(map[string]any{"error": err.Error()})
		}
		return err
	}

	var wf *parser.Workflow
	for i := range spec.Workflows {
		if spec.Workflows[i].WorkflowID == workflowID {
			wf = &spec.Workflows[i]
			break
		}
	}

	if jsonFlag {
		inputs := map[string]inputDetail{}
		if wf.Inputs != nil {
			for name, prop := range wf.Inputs.Properties {
				req := false
				for _, r := range wf.Inputs.Required {
					if r == name {
						req = true
						break
					}
				}
				inputs[name] = inputDetail{
					Type:        prop.Type,
					Required:    req,
					Description: prop.Description,
				}
			}
		}

		return outputJSON(workflowDetail{
			ID:      wf.WorkflowID,
			File:    filename,
			Title:   spec.Info.Title,
			Summary: wf.Summary,
			Steps:   len(wf.Steps),
			Inputs:  inputs,
			Outputs: mapKeys(wf.Outputs),
			Sources: buildSources(spec),
		})
	}

	fmt.Printf("Workflow: %s\n", workflowID)
	fmt.Printf("File:     %s\n", filename)
	fmt.Printf("Title:    %s\n", spec.Info.Title)
	if wf.Summary != "" {
		fmt.Printf("Summary:  %s\n", wf.Summary)
	}
	fmt.Printf("Steps:    %d\n", len(wf.Steps))
	fmt.Println()

	if wf.Inputs != nil && len(wf.Inputs.Properties) > 0 {
		fmt.Println("Inputs:")
		for name, prop := range wf.Inputs.Properties {
			required := ""
			for _, r := range wf.Inputs.Required {
				if r == name {
					required = " (required)"
					break
				}
			}
			desc := ""
			if prop.Description != "" {
				desc = " - " + prop.Description
			}
			fmt.Printf("  --input %s=<%s>%s%s\n", name, prop.Type, required, desc)
		}
		fmt.Println()
	}

	if len(wf.Outputs) > 0 {
		fmt.Println("Outputs:")
		for name := range wf.Outputs {
			fmt.Printf("  %s\n", name)
		}
	}

	return nil
}

// ── helpers ─────────────────────────────────────────────────────────────

func findWorkflow(dir, workflowID string) (*parser.ArazzoSpec, string, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, "", fmt.Errorf("reading directory %q: %w", dir, err)
	}

	var matches []string
	var matchSpec *parser.ArazzoSpec
	var matchFile string

	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".yaml") {
			continue
		}
		specPath := filepath.Join(dir, entry.Name())
		spec, err := parser.Parse(specPath)
		if err != nil {
			continue
		}
		for _, wf := range spec.Workflows {
			if wf.WorkflowID == workflowID {
				matches = append(matches, entry.Name())
				matchSpec = spec
				matchFile = entry.Name()
			}
		}
	}

	if len(matches) == 0 {
		return nil, "", fmt.Errorf("workflow %q not found in %s", workflowID, dir)
	}
	if len(matches) > 1 {
		return nil, "", fmt.Errorf("workflow %q found in multiple files: %v", workflowID, matches)
	}

	return matchSpec, matchFile, nil
}

func buildWorkflowInfo(wf *parser.Workflow) workflowInfo {
	inputs := []string{}
	if wf.Inputs != nil {
		for name := range wf.Inputs.Properties {
			inputs = append(inputs, name)
		}
	}
	return workflowInfo{
		ID:      wf.WorkflowID,
		Summary: wf.Summary,
		Inputs:  inputs,
		Outputs: mapKeys(wf.Outputs),
	}
}

func buildSources(spec *parser.ArazzoSpec) []sourceInfo {
	sources := []sourceInfo{}
	for _, sd := range spec.SourceDescriptions {
		sources = append(sources, sourceInfo{Name: sd.Name, URL: sd.URL, Type: sd.Type})
	}
	return sources
}

func parseInputValue(s string) any {
	// Expand environment variables ($VAR or ${VAR})
	if strings.HasPrefix(s, "$") {
		varName := strings.TrimPrefix(s, "$")
		varName = strings.Trim(varName, "{}")
		if val, ok := os.LookupEnv(varName); ok {
			s = val
		}
	}

	// Try float - must consume entire string
	var f float64
	var n int
	if _, err := fmt.Sscanf(s, "%f%n", &f, &n); err == nil && n == len(s) {
		return f
	}
	if s == "true" {
		return true
	}
	if s == "false" {
		return false
	}
	return s
}

func mapKeys(m map[string]string) []string {
	k := make([]string, 0, len(m))
	for key := range m {
		k = append(k, key)
	}
	return k
}

func outputJSON(v any) error {
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	return enc.Encode(v)
}

func loadEnvFile(filename string) {
	file, err := os.Open(filename)
	if err != nil {
		return
	}
	defer func() { _ = file.Close() }()

	scanner := bufio.NewScanner(file)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		parts := strings.SplitN(line, "=", 2)
		if len(parts) == 2 {
			key := strings.TrimSpace(parts[0])
			value := strings.TrimSpace(parts[1])
			value = strings.Trim(value, "\"'")
			_ = os.Setenv(key, value)
		}
	}
}
