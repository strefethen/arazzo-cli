import * as vscode from "vscode";

export class ArazzoAdapterDescriptorFactory
  implements vscode.DebugAdapterDescriptorFactory, vscode.Disposable
{
  createDebugAdapterDescriptor(
    session: vscode.DebugSession
  ): vscode.ProviderResult<vscode.DebugAdapterDescriptor> {
    const command = asString(session.configuration.runtimeExecutable) ?? "cargo";
    const args = asStringArray(session.configuration.runtimeArgs) ?? [
      "run",
      "-p",
      "arazzo-debug-adapter",
      "--quiet",
      "--"
    ];
    const options: vscode.DebugAdapterExecutableOptions = {};
    return new vscode.DebugAdapterExecutable(command, args, options);
  }

  dispose(): void {}
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function asStringArray(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const arr = value.filter((item): item is string => typeof item === "string");
  return arr.length > 0 ? arr : undefined;
}
