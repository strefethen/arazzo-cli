import * as os from "node:os";
import * as vscode from "vscode";

const sessionCountKey = "feedback.completedSessions";
const invitationShownKey = "feedback.invitationShown";
const sendFeedback = "Send Feedback";
const neverAsk = "Don’t Ask Again";

export function registerFeedback(context: vscode.ExtensionContext): void {
  const reportError = (error: unknown): void => {
    void vscode.window.showErrorMessage(`Arazzo feedback: ${error instanceof Error ? error.message : String(error)}`);
  };
  const openIssue = async (bug: boolean): Promise<void> => {
    const body = [
      bug ? "## What went wrong?" : "## What would make Arazzo Debugger more useful?",
      "", "", ...(bug ? ["## Steps to reproduce", "", "", "## Expected behavior", "", ""] : []),
      "## Environment",
      `- Arazzo Debugger: ${context.extension.packageJSON.version}`,
      `- VS Code: ${vscode.version}`,
      `- OS: ${os.type()} ${os.release()} (${os.arch()})`,
      "", "Please review before submitting. Remove sensitive information from any examples or logs you add.",
    ].join("\n");
    const query = new URLSearchParams({ title: bug ? "[Debugger bug] " : "[Debugger feedback] ", body });
    const opened = await vscode.env.openExternal(vscode.Uri.parse(`https://github.com/strefethen/arazzo-cli/issues/new?${query}`));
    if (!opened) {
      throw new Error("Could not open GitHub. Visit https://github.com/strefethen/arazzo-cli/issues to share feedback.");
    }
  };

  // Serialize local state updates when multiple debug sessions end together.
  let pending = Promise.resolve();
  const seenSessions = new Set<string>();
  context.subscriptions.push(
    vscode.commands.registerCommand("arazzo.sendFeedback", () => openIssue(false).catch(reportError)),
    vscode.commands.registerCommand("arazzo.reportBug", () => openIssue(true).catch(reportError)),
    vscode.debug.onDidTerminateDebugSession((session) => {
      if (session.type !== "arazzo" || session.configuration.noDebug ||
          context.globalState.get<boolean>(invitationShownKey, false) || seenSessions.has(session.id)) {
        return;
      }
      seenSessions.add(session.id);
      pending = pending.then(async () => {
        if (context.globalState.get<boolean>(invitationShownKey, false)) {
          return;
        }
        const count = context.globalState.get<number>(sessionCountKey, 0) + 1;
        await context.globalState.update(sessionCountKey, count);
        if (count < 3) {
          return;
        }
        // Persist before showing: closing the invitation also dismisses it permanently.
        await context.globalState.update(invitationShownKey, true);
        const answer = await vscode.window.showInformationMessage(
          "How is Arazzo Debugger working for you? Bugs, missing features, and workflow feedback help shape the next release.",
          sendFeedback,
          neverAsk
        );
        if (answer === sendFeedback) {
          await openIssue(false);
        }
      }).catch(reportError);
    })
  );
}
