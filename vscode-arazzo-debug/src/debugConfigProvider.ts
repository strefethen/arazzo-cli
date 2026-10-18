import * as vscode from "vscode";

export class ArazzoDebugConfigurationProvider
  implements vscode.DebugConfigurationProvider
{
  constructor(private readonly channel: vscode.OutputChannel) {}

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
      const message = `Cannot start "${resolved.name}": no Arazzo spec is selected. Open an .arazzo.yaml file or set 'spec' to its path in .vscode/launch.json.`;
      this.channel.appendLine(message);
      this.channel.show(true);
      void vscode.window.showErrorMessage(message);
      return undefined;
    }

    return resolved;
  }
}
