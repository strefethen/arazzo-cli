import * as vscode from "vscode";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

function getBundledBinaryPath(extensionPath: string): string | undefined {
  const ext = os.platform() === "win32" ? ".exe" : "";
  const binPath = path.join(
    extensionPath,
    "bin",
    `arazzo-debug-adapter${ext}`
  );
  return fs.existsSync(binPath) ? binPath : undefined;
}

export class ArazzoAdapterDescriptorFactory
  implements vscode.DebugAdapterDescriptorFactory, vscode.Disposable
{
  constructor(private readonly extensionPath: string) {}

  createDebugAdapterDescriptor(
    session: vscode.DebugSession
  ): vscode.ProviderResult<vscode.DebugAdapterDescriptor> {
    const runtimeExecutable = asString(
      session.configuration.runtimeExecutable
    );

    if (runtimeExecutable) {
      // Dev/override mode: user controls the full launch
      const args = asStringArray(session.configuration.runtimeArgs) ?? [];
      const cwd = asString(session.configuration.runtimeCwd);
      const options: vscode.DebugAdapterExecutableOptions = { cwd };
      return new vscode.DebugAdapterExecutable(runtimeExecutable, args, options);
    }

    // Default mode: resolve bundled binary
    const binPath = getBundledBinaryPath(this.extensionPath);
    if (!binPath) {
      void vscode.window.showErrorMessage(
        "Arazzo Debug: bundled debug adapter binary not found. " +
          "If running from source, set 'runtimeExecutable' in your launch config."
      );
      return undefined;
    }

    const cwd =
      vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ??
      (asString(session.configuration.spec)
        ? path.dirname(session.configuration.spec as string)
        : undefined);

    const options: vscode.DebugAdapterExecutableOptions = { cwd };
    return new vscode.DebugAdapterExecutable(binPath, [], options);
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
