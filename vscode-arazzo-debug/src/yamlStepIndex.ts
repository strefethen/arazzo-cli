export interface StepLocation {
  workflowId: string;
  stepId: string;
  line: number;
}

export interface WorkflowStepIndex {
  steps: StepLocation[];
}

// Placeholder parser: line-to-step extraction will be expanded in the next debugger pass.
export function buildWorkflowStepIndex(_text: string): WorkflowStepIndex {
  return { steps: [] };
}
