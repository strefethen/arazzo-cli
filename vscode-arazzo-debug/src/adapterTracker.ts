import type * as vscode from "vscode";

interface DapEventShape {
  type?: string;
  event?: string;
  command?: string;
  success?: boolean;
  message?: string;
  body?: {
    category?: string;
    output?: string;
    exitCode?: number;
    data?: { kind?: string };
    error?: { format?: string; variables?: Record<string, string> };
    stackFrames?: Array<{ name: string; line: number; source?: { path?: string } }>;
    breakpoints?: Array<{ verified?: boolean; line?: number; message?: string }>;
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
    let ended = false;
    let stopping = false;
    let outcomeReported = false;
    let unresolvedOutputs = 0;
    let lastCommand = "session startup";
    let lastFrame: string | undefined;
    const context = (): string =>
      `spec: ${session.configuration?.spec ?? "not supplied"}; workflow: ${session.configuration?.workflowId || "first workflow"}; last request: ${lastCommand}${lastFrame ? `; last observed frame: ${lastFrame}` : ""}`;

    return {
      onWillStartSession(): void {
        channel.appendLine(`[${stamp()}] session starting: ${session.name} (${context()})`);
      },
      onWillReceiveMessage(message: unknown): void {
        const request = message as DapEventShape;
        if (request.type === "request" && request.command) {
          lastCommand = request.command;
          if (request.command === "disconnect") {
            stopping = true;
          }
        }
      },
      onDidSendMessage(message: unknown): void {
        const dap = message as DapEventShape;
        if (dap.type === "response") {
          if (dap.success === false) {
            const error = dap.body?.error;
            const detail = error?.format?.replace(/\{([^{}]+)\}/g,
              (placeholder, key: string) => error.variables?.[key] ?? placeholder)
              || dap.message || "adapter returned no diagnostic; report this command and session context";
            channel.appendLine(
              `[${stamp()}] ${dap.command ?? "request"} failed: ${detail} (${context()})`
            );
            channel.show(true);
          } else if (dap.command === "setBreakpoints") {
            for (const breakpoint of dap.body?.breakpoints ?? []) {
              if (breakpoint.verified === false) {
                channel.appendLine(
                  `[${stamp()}] breakpoint not installed: ${breakpoint.message ?? `line ${breakpoint.line ?? "unknown"}; adapter supplied no reason`} (${context()})`
                );
                channel.show(true);
              }
            }
          } else if (dap.command === "stackTrace") {
            const frame = dap.body?.stackFrames?.[0];
            lastFrame = frame
              ? `${frame.name} (${frame.source?.path ?? "unknown source"}:${frame.line})`
              : undefined;
          }
          return;
        }
        if (dap.type !== "event") {
          return;
        }
        if (dap.event === "output" && dap.body?.data?.kind === "unresolvedOutput") {
          unresolvedOutputs += 1;
          channel.append(`[${stamp()}] ${dap.body.output ?? ""}`);
          channel.show(true);
        } else if (dap.event === "output" && dap.body?.category === "stderr") {
          // Adapter stderr output events are newline-terminated already.
          channel.append(`[${stamp()}] ${dap.body.output ?? ""}`);
          channel.appendLine(`[${stamp()}] failure context: ${context()}`);
          channel.show(true);
        } else if (dap.event === "terminated") {
          if (ended) {
            return;
          }
          ended = true;
          if (!outcomeReported) {
            channel.appendLine(`[${stamp()}] ${stopping ? "debug session stopped by request" : "workflow ended; adapter did not report an exit status"}`);
          }
        } else if (dap.event === "exited") {
          const code = dap.body?.exitCode;
          outcomeReported = true;
          const success = unresolvedOutputs > 0
            ? `workflow completed with ${unresolvedOutputs} unresolved output(s) (exit code 0)`
            : "workflow completed successfully (exit code 0)";
          channel.appendLine(
            `[${stamp()}] ${code === 0 ? success : `workflow failed (exit code ${code ?? "unknown"}); see diagnostics above`}`
          );
        }
      },
      onError(error: Error): void {
        // VS Code emits this when stdout closes, including normal shutdown.
        if ((ended || stopping) && error.message === "read error") {
          return;
        }
        const detail = error.message === "read error"
          ? "adapter output stream closed unexpectedly before termination; check the process exit code or signal below"
          : error.message;
        channel.appendLine(`[${stamp()}] adapter error: ${detail} (${context()})`);
        channel.show(true);
      },
      onExit(code: number | undefined, signal: string | undefined): void {
        if (stopping && (signal === "SIGTERM" || signal === "SIGKILL")) {
          channel.appendLine(`[${stamp()}] adapter stopped by request (signal ${signal})`);
          return;
        }
        if (signal || (code !== undefined && code !== 0)) {
          const suffix = signal ? `, signal ${signal}` : "";
          channel.appendLine(
            `[${stamp()}] adapter process exited with code ${code ?? "unknown"}${suffix} (${context()})`
          );
          channel.show(true);
        } else if (!ended && !stopping) {
          channel.appendLine(`[${stamp()}] adapter exited without a workflow termination event (code ${code ?? "unknown"}; ${context()})`);
          channel.show(true);
        }
      },
      onWillStopSession(): void {
        ended = true;
        stopping = true;
        channel.appendLine(`[${stamp()}] session ended: ${session.name}`);
      },
    };
  }
}
