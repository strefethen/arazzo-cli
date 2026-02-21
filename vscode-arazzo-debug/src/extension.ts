import * as vscode from "vscode";
import { ArazzoAdapterDescriptorFactory } from "./adapterClient";
import { ArazzoDebugConfigurationProvider } from "./debugConfigProvider";

export function activate(context: vscode.ExtensionContext): void {
  const provider = new ArazzoDebugConfigurationProvider();
  const factory = new ArazzoAdapterDescriptorFactory(context.extensionPath);

  context.subscriptions.push(
    vscode.debug.registerDebugConfigurationProvider("arazzo", provider),
    vscode.debug.registerDebugAdapterDescriptorFactory("arazzo", factory),
    factory
  );
}

export function deactivate(): void {}
