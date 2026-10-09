import assert from "node:assert/strict";
import { test } from "node:test";

import { findEvaluatableExpression } from "../evaluatableExpression";

test("selects the response object inside a simple condition value", () => {
  const line =
    "        - condition: $response.body#/features/0/properties/display_name != null";
  const result = findEvaluatableExpression(line, line.indexOf("response"));
  assert.deepEqual(result, {
    start: line.indexOf("$response"),
    end: line.indexOf(".body"),
    expression: "$response",
  });
});

test("selects the response body inside a simple condition value", () => {
  const line =
    "        - condition: $response.body#/features/0/properties/display_name != null";
  const result = findEvaluatableExpression(line, line.indexOf("body"));
  assert.deepEqual(result, {
    start: line.indexOf("$response"),
    end: line.indexOf("#/"),
    expression: "$response.body",
  });
});

test("selects the JSON Pointer segment under the cursor", () => {
  const line =
    "        - condition: $response.body#/features/0/properties/display_name != null";
  const result = findEvaluatableExpression(line, line.indexOf("features"));
  assert.deepEqual(result, {
    start: line.indexOf("$response"),
    end: line.indexOf("/0"),
    expression: "$response.body#/features",
  });
});

test("selects the scalar JSON Pointer value inside a simple condition value", () => {
  const line =
    "        - condition: $response.body#/features/0/properties/display_name != null";
  const result = findEvaluatableExpression(line, line.indexOf("display_name"));
  assert.deepEqual(result, {
    start: line.indexOf("$response"),
    end: line.indexOf(" !="),
    expression: "$response.body#/features/0/properties/display_name",
  });
});

test("selects the whole condition when hovering the condition operator", () => {
  const line =
    "        - condition: $response.body#/features/0/properties/display_name != null";
  const result = findEvaluatableExpression(line, line.indexOf("!="));
  assert.deepEqual(result, {
    start: line.indexOf("$response"),
    end: line.length,
    expression: "$response.body#/features/0/properties/display_name != null",
  });
});

test("selects the runtime operand inside a quoted condition value", () => {
  const line =
    '        - condition: "$response.body#/features/0/properties/display_name != null"';
  const result = findEvaluatableExpression(line, line.indexOf("display_name"));
  assert.deepEqual(result, {
    start: line.indexOf("$response"),
    end: line.indexOf(" !="),
    expression: "$response.body#/features/0/properties/display_name",
  });
});

test("selects an unquoted runtime expression scalar", () => {
  const line = "            value: $inputs.address";
  const result = findEvaluatableExpression(line, line.indexOf("address"));
  assert.deepEqual(result, {
    start: line.indexOf("$inputs"),
    end: line.length,
    expression: "$inputs.address",
  });
});

test("selects quoted XPath selector content without YAML quotes", () => {
  const line =
    "          title_1: {context: $response.body, selector: '//item[1]/title', type: {type: xpath, version: xpath-10}}";
  const result = findEvaluatableExpression(line, line.indexOf("item[1]"));
  assert.deepEqual(result, {
    start: line.indexOf("//item"),
    end: line.indexOf("', type"),
    expression: "//item[1]/title",
  });
});

test("does not classify operation paths as XPath hovers", () => {
  const line = "        operationPath: /geo";
  assert.equal(findEvaluatableExpression(line, line.indexOf("geo")), undefined);
});

test("selects inline context runtime expression", () => {
  const line =
    "          title_1: {context: $response.body, selector: '//item[1]/title'}";
  const result = findEvaluatableExpression(line, line.indexOf("body"));
  assert.deepEqual(result, {
    start: line.indexOf("$response"),
    end: line.indexOf(", selector"),
    expression: "$response.body",
  });
});
