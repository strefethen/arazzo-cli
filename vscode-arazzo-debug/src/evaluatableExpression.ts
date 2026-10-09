export interface EvaluatableExpressionSpan {
  start: number;
  end: number;
  expression: string;
}

const EXPRESSION_KEYS = new Set(["condition", "context", "selector", "value"]);

export function findEvaluatableExpression(
  line: string,
  character: number
): EvaluatableExpressionSpan | undefined {
  return (
    findInlineExpression(line, character) ??
    findQuotedExpression(line, character) ??
    findKeyedScalarExpression(line, character)
  );
}

function findQuotedExpression(
  line: string,
  character: number
): EvaluatableExpressionSpan | undefined {
  for (let start = 0; start < line.length; start += 1) {
    const quote = line[start];
    if (quote !== "'" && quote !== '"') {
      continue;
    }

    let end = start + 1;
    while (end < line.length) {
      if (line[end] === quote) {
        if (quote === "'" && line[end + 1] === "'") {
          end += 2;
          continue;
        }
        break;
      }
      end += 1;
    }
    if (end >= line.length || character < start + 1 || character > end) {
      continue;
    }

    const key = findNearestKeyBefore(line, start)?.name;
    const content = trimSpan(line, start + 1, end);
    if (!content || !isExpressionForKey(key, content.expression)) {
      return undefined;
    }
    return content;
  }

  return undefined;
}

function findKeyedScalarExpression(
  line: string,
  character: number
): EvaluatableExpressionSpan | undefined {
  const key = findNearestKeyBefore(line, character);
  if (!key || !EXPRESSION_KEYS.has(key.name)) {
    return undefined;
  }

  const valueStart = skipWhitespace(line, key.valueStart);
  if (character < valueStart) {
    return undefined;
  }
  const valueEnd = findScalarEnd(line, valueStart);
  if (character > valueEnd) {
    return undefined;
  }

  const value = trimSpan(line, valueStart, valueEnd);
  if (!value || !isExpressionForKey(key.name, value.expression)) {
    return undefined;
  }
  return value;
}

function findInlineExpression(
  line: string,
  character: number
): EvaluatableExpressionSpan | undefined {
  const dollar = findTokenStart(line, character, "$");
  if (dollar !== undefined) {
    const end = findExpressionTokenEnd(line, dollar);
    const key = findNearestKeyBefore(line, dollar);
    const expression = line.slice(dollar, end);
    if (isExpressionForKey(key, expression)) {
      return narrowRuntimeExpression(line, dollar, end, character);
    }
  }

  const slash = findTokenStart(line, character, "//");
  if (slash !== undefined) {
    const key = findNearestKeyBefore(line, slash);
    const end = findExpressionTokenEnd(line, slash);
    const expression = line.slice(slash, end);
    if (isExpressionForKey(key, expression)) {
      return { start: slash, end, expression };
    }
  }

  return undefined;
}

function narrowRuntimeExpression(
  line: string,
  start: number,
  end: number,
  character: number
): EvaluatableExpressionSpan {
  const hash = line.indexOf("#", start);
  const runtimeEnd = hash === -1 || hash >= end ? end : hash;
  if (character < runtimeEnd || hash === -1 || hash >= end) {
    return prefixThroughDelimitedSegment(line, start, runtimeEnd, character, ".");
  }

  if (character <= hash) {
    return prefixThroughDelimitedSegment(line, start, runtimeEnd, character, ".");
  }

  return prefixThroughJsonPointerSegment(line, start, hash, end, character);
}

function prefixThroughDelimitedSegment(
  line: string,
  start: number,
  end: number,
  character: number,
  delimiter: string
): EvaluatableExpressionSpan {
  let segmentStart = start;
  while (segmentStart < end) {
    const nextDelimiter = line.indexOf(delimiter, segmentStart);
    const segmentEnd =
      nextDelimiter === -1 || nextDelimiter > end ? end : nextDelimiter;
    if (character <= segmentEnd) {
      return spanFor(line, start, segmentEnd);
    }
    segmentStart = segmentEnd + delimiter.length;
  }

  return spanFor(line, start, end);
}

function prefixThroughJsonPointerSegment(
  line: string,
  start: number,
  hash: number,
  end: number,
  character: number
): EvaluatableExpressionSpan {
  let slash = hash + 1;
  let previousSegmentEnd = hash;
  while (slash < end) {
    if (line[slash] !== "/") {
      return spanFor(line, start, end);
    }

    if (character <= slash) {
      return spanFor(line, start, previousSegmentEnd);
    }

    const nextSlash = line.indexOf("/", slash + 1);
    const segmentEnd = nextSlash === -1 || nextSlash > end ? end : nextSlash;
    if (character <= segmentEnd) {
      return spanFor(line, start, segmentEnd);
    }

    previousSegmentEnd = segmentEnd;
    slash = segmentEnd;
  }

  return spanFor(line, start, end);
}

function spanFor(
  line: string,
  start: number,
  end: number
): EvaluatableExpressionSpan {
  return { start, end, expression: line.slice(start, end) };
}

interface KeyMatch {
  name: string;
  valueStart: number;
}

function findNearestKeyBefore(
  line: string,
  before: number
): KeyMatch | undefined {
  const keyPattern = /([A-Za-z_][A-Za-z0-9_-]*)\s*:\s*/g;
  let best: KeyMatch | undefined;
  for (let match = keyPattern.exec(line); match; match = keyPattern.exec(line)) {
    const valueStart = match.index + match[0].length;
    if (valueStart > before) {
      break;
    }
    best = { name: match[1], valueStart };
  }
  return best;
}

function isExpressionForKey(
  key: KeyMatch | string | undefined,
  expression: string
): boolean {
  const keyName = typeof key === "string" ? key : key?.name;
  if (!keyName || !EXPRESSION_KEYS.has(keyName)) {
    return expression.startsWith("$");
  }
  if (keyName === "selector") {
    return (
      expression.startsWith("$") ||
      expression.startsWith("/") ||
      /^[A-Za-z_][A-Za-z0-9_-]*\s*\(/.test(expression)
    );
  }
  if (keyName === "context" || keyName === "value") {
    return expression.startsWith("$");
  }
  return (
    expression.startsWith("$") ||
    expression.startsWith("/") ||
    expression === "true" ||
    expression === "false" ||
    expression === "null" ||
    /(?:==|!=|<=|>=|&&|\|\||\s[<>]\s)/.test(expression)
  );
}

function findTokenStart(
  line: string,
  character: number,
  marker: string
): number | undefined {
  for (let start = character; start >= 0; start -= 1) {
    if (line.startsWith(marker, start)) {
      const previous = start === 0 ? "" : line[start - 1];
      if (!previous || isTokenBoundary(previous)) {
        return start;
      }
    }
    if (isTokenBoundary(line[start])) {
      return undefined;
    }
  }
  return undefined;
}

function findExpressionTokenEnd(line: string, start: number): number {
  let end = start;
  while (end < line.length && !isTokenBoundary(line[end])) {
    end += 1;
  }
  return end;
}

function isTokenBoundary(character: string): boolean {
  return /\s|[,{}'"]/.test(character);
}

function findScalarEnd(line: string, start: number): number {
  for (let index = start; index < line.length; index += 1) {
    const character = line[index];
    if (character === "," || character === "}") {
      return index;
    }
    if (character === "#" && (index === start || /\s/.test(line[index - 1]))) {
      return index;
    }
  }
  return line.length;
}

function skipWhitespace(line: string, start: number): number {
  let index = start;
  while (index < line.length && /\s/.test(line[index])) {
    index += 1;
  }
  return index;
}

function trimSpan(
  line: string,
  start: number,
  end: number
): EvaluatableExpressionSpan | undefined {
  let trimmedStart = start;
  let trimmedEnd = end;
  while (trimmedStart < trimmedEnd && /\s/.test(line[trimmedStart])) {
    trimmedStart += 1;
  }
  while (trimmedEnd > trimmedStart && /\s/.test(line[trimmedEnd - 1])) {
    trimmedEnd -= 1;
  }
  if (trimmedStart >= trimmedEnd) {
    return undefined;
  }
  return {
    start: trimmedStart,
    end: trimmedEnd,
    expression: line.slice(trimmedStart, trimmedEnd),
  };
}
