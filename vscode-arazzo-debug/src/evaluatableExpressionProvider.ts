import * as vscode from "vscode";

import { findEvaluatableExpression } from "./evaluatableExpression";

export class ArazzoEvaluatableExpressionProvider
  implements vscode.EvaluatableExpressionProvider
{
  provideEvaluatableExpression(
    document: vscode.TextDocument,
    position: vscode.Position
  ): vscode.ProviderResult<vscode.EvaluatableExpression> {
    const span = findEvaluatableExpression(
      document.lineAt(position.line).text,
      position.character
    );
    if (!span) {
      return undefined;
    }

    return new vscode.EvaluatableExpression(
      new vscode.Range(position.line, span.start, position.line, span.end),
      span.expression
    );
  }
}
