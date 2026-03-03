import * as vscode from "vscode";

export class ArazzoDebugConfigurationProvider
  implements vscode.DebugConfigurationProvider
{
  resolveDebugConfiguration(
    folder: vscode.WorkspaceFolder | undefined,
    config: vscode.DebugConfiguration
  ): vscode.ProviderResult<vscode.DebugConfiguration> {
    const resolved = { ...config };
    if (!resolved.type) {
      resolved.type = "arazzo";
    }
    if (!resolved.request) {
      resolved.request = "launch";
    }
    if (!resolved.name) {
      resolved.name = "Debug Arazzo Workflow";
    }
    if (!resolved.spec && vscode.window.activeTextEditor) {
      resolved.spec = vscode.window.activeTextEditor.document.fileName;
    }
    if (!resolved.inputs) {
      resolved.inputs = {};
    }
    if (typeof resolved.stopOnEntry !== "boolean") {
      resolved.stopOnEntry = false;
    }

    if (!resolved.spec) {
      void vscode.window.showErrorMessage(
        "Arazzo debug config requires 'spec'."
      );
      return undefined;
    }

    return resolved;
  }
}
