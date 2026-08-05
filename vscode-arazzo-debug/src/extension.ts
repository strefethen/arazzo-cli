import * as vscode from "vscode";
import { ArazzoAdapterDescriptorFactory } from "./adapterClient";
import { ArazzoAdapterTrackerFactory } from "./adapterTracker";
import { ArazzoDebugConfigurationProvider } from "./debugConfigProvider";

export function activate(context: vscode.ExtensionContext): void {
  const channel = vscode.window.createOutputChannel("Arazzo Debug");
  const provider = new ArazzoDebugConfigurationProvider();
  const factory = new ArazzoAdapterDescriptorFactory(
    context.extensionPath,
    channel
  );

  context.subscriptions.push(
    channel,
    vscode.debug.registerDebugConfigurationProvider("arazzo", provider),
    vscode.debug.registerDebugAdapterDescriptorFactory("arazzo", factory),
    vscode.debug.registerDebugAdapterTrackerFactory(
      "arazzo",
      new ArazzoAdapterTrackerFactory(channel)
    ),
    factory
  );
}

export function deactivate(): void {}
