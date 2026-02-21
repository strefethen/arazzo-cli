import * as vscode from "vscode";
import * as path from "node:path";

export class ArazzoAdapterDescriptorFactory
  implements vscode.DebugAdapterDescriptorFactory, vscode.Disposable
{
  constructor(private readonly extensionPath: string) {}

  createDebugAdapterDescriptor(
    session: vscode.DebugSession
  ): vscode.ProviderResult<vscode.DebugAdapterDescriptor> {
    const repoRoot = path.resolve(this.extensionPath, "..");
    const manifestPath = path.join(repoRoot, "Cargo.toml");
    const command = asString(session.configuration.runtimeExecutable) ?? "cargo";
    const args = asStringArray(session.configuration.runtimeArgs) ?? [
      "run",
      "--manifest-path",
      manifestPath,
      "-p",
      "arazzo-debug-adapter",
      "--quiet",
      "--"
    ];
    const cwd = asString(session.configuration.runtimeCwd) ?? repoRoot;
    const options: vscode.DebugAdapterExecutableOptions = { cwd };
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
