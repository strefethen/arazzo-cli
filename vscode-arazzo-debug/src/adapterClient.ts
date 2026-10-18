import * as vscode from "vscode";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

function getBundledBinaryPath(extensionPath: string): string {
  const ext = os.platform() === "win32" ? ".exe" : "";
  const binPath = path.join(
    extensionPath,
    "bin",
    `arazzo-debug-adapter${ext}`
  );
  return binPath;
}

export class ArazzoAdapterDescriptorFactory
  implements vscode.DebugAdapterDescriptorFactory, vscode.Disposable
{
  constructor(
    private readonly extensionPath: string,
    private readonly channel: vscode.OutputChannel
  ) {}

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
      if (cwd && !this.validatePath(cwd, "directory", session.name)) {
        return undefined;
      }
      // Bare commands are resolved through PATH by the process launcher.
      if ((path.isAbsolute(runtimeExecutable) || /[/\\]/.test(runtimeExecutable)) &&
          !this.validatePath(path.resolve(cwd ?? process.cwd(), runtimeExecutable), "executable", session.name)) {
        return undefined;
      }
      const options: vscode.DebugAdapterExecutableOptions = { cwd };
      this.channel.appendLine(
        `launching adapter (runtimeExecutable override): ${runtimeExecutable}${cwd ? `; working directory: ${cwd}` : ""}`
      );
      return new vscode.DebugAdapterExecutable(runtimeExecutable, args, options);
    }

    // Default mode: resolve bundled binary
    const binPath = getBundledBinaryPath(this.extensionPath);
    if (!fs.existsSync(binPath)) {
      const message =
        `Cannot start "${session.name}": bundled debug adapter is missing at "${binPath}". ` +
        "Reinstall the Arazzo Debugger extension, or build the adapter and set 'runtimeExecutable' to that executable in .vscode/launch.json.";
      return this.reportError(message);
    }
    if (!this.validatePath(binPath, "executable", session.name)) {
      return undefined;
    }
    this.channel.appendLine(`launching bundled adapter: ${binPath}`);

    const cwd =
      vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ??
      (asString(session.configuration.spec)
        ? path.dirname(session.configuration.spec as string)
        : undefined);

    const options: vscode.DebugAdapterExecutableOptions = { cwd };
    return new vscode.DebugAdapterExecutable(binPath, [], options);
  }

  private reportError(message: string): undefined {
    this.channel.appendLine(message);
    this.channel.show(true);
    void vscode.window.showErrorMessage(message);
    return undefined;
  }

  private validatePath(target: string, kind: "directory" | "executable", sessionName: string): boolean {
    try {
      const stat = fs.statSync(target);
      if (kind === "directory" ? !stat.isDirectory() : !stat.isFile()) {
        this.reportError(`Cannot start "${sessionName}": "${target}" is not a ${kind === "directory" ? "directory" : "file"}. Correct '${kind === "directory" ? "runtimeCwd" : "runtimeExecutable"}' in .vscode/launch.json.`);
        return false;
      }
      fs.accessSync(target, fs.constants.X_OK);
      return true;
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      const action = kind === "directory"
        ? "Set 'runtimeCwd' to an existing accessible directory in .vscode/launch.json."
        : "Build or install the adapter and set 'runtimeExecutable' to its executable path in .vscode/launch.json. For this repository, run 'cargo build -p arazzo-debug-adapter' for target/debug or add '--release' for target/release.";
      this.reportError(`Cannot start "${sessionName}": cannot access ${kind} "${target}": ${detail}. ${action}`);
      return false;
    }
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
