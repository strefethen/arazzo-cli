import * as vscode from "vscode";
import { buildWorkflowStepIndex, StepLocation } from "./yamlStepIndex";

export interface MappedBreakpoint {
  line: number;
  location?: StepLocation;
}

// Placeholder mapping: currently returns line-only records until YAML indexing is added.
export function mapBreakpoints(
  document: vscode.TextDocument,
  breakpoints: readonly vscode.SourceBreakpoint[]
): MappedBreakpoint[] {
  const _index = buildWorkflowStepIndex(document.getText());
  return breakpoints.map((bp) => ({
    line: bp.location.range.start.line + 1
  }));
}
