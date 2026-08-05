import * as vscode from "vscode";

interface DapEventShape {
  type?: string;
  event?: string;
  body?: {
    category?: string;
    output?: string;
    exitCode?: number;
  };
}

/**
 * Mirrors debug-session lifecycle, workflow failures, and adapter process
 * problems into the "Arazzo Debug" output channel, so a session that ends
 * abnormally always leaves a user-visible trace (issue #2 follow-up: sessions
 * ended with no output anywhere in the UI).
 */
export class ArazzoAdapterTrackerFactory
  implements vscode.DebugAdapterTrackerFactory
{
  constructor(private readonly channel: vscode.OutputChannel) {}

  createDebugAdapterTracker(
    session: vscode.DebugSession
  ): vscode.ProviderResult<vscode.DebugAdapterTracker> {
    const channel = this.channel;
    const stamp = (): string => new Date().toISOString();

    return {
      onWillStartSession(): void {
        channel.appendLine(`[${stamp()}] session starting: ${session.name}`);
      },
      onDidSendMessage(message: unknown): void {
        const dap = message as DapEventShape;
        if (dap.type !== "event") {
          return;
        }
        if (dap.event === "output" && dap.body?.category === "stderr") {
          // Adapter stderr output events are newline-terminated already.
          channel.append(`[${stamp()}] ${dap.body.output ?? ""}`);
        } else if (dap.event === "exited") {
          const code = dap.body?.exitCode;
          channel.appendLine(
            `[${stamp()}] workflow exited with code ${code ?? "unknown"}`
          );
        }
      },
      onError(error: Error): void {
        channel.appendLine(`[${stamp()}] adapter error: ${error.message}`);
        channel.show(true);
      },
      onExit(code: number | undefined, signal: string | undefined): void {
        if (code !== undefined && code !== 0) {
          const suffix = signal ? `, signal ${signal}` : "";
          channel.appendLine(
            `[${stamp()}] adapter process exited with code ${code}${suffix}`
          );
          channel.show(true);
        }
      },
      onWillStopSession(): void {
        channel.appendLine(`[${stamp()}] session ended: ${session.name}`);
      },
    };
  }
}
