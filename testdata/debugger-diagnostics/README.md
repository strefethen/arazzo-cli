# Debugger diagnostics demonstrations

Open `debugger-diagnostics.code-workspace` in VS Code. These configurations
use the locally built adapter and `dryRun: true`; no server is needed and
no HTTP request is sent. The OpenAPI server uses the reserved `.invalid` domain.

- **Missing outputs and watches:** Start debugging, then evaluate
  `$inputs.alerts#/features/0/properties/headline`, `missing_alias`, and
  `$inputs.explicitNull`. The first two must report errors with the expression,
  workflow, and step; the last must display `null`. Continue to see two step
  output diagnostics and one workflow output diagnostic. Completion must say
  there were unresolved outputs. `explicit_null` must not be flagged.
  The response expression intentionally points into the empty dry-run response;
  the input expression demonstrates an actual `features: []` object.
- **Breakpoint conditions:** In `outputs.arazzo.yaml`, put a conditional
  breakpoint on `stepId: check-active-alerts`. Try `$inputs.alerts ==`:
  the breakpoint must be unverified with the syntax error. Try
  `($inputs.explicitNull).missing == 1`: it must report the evaluation error
  with workflow, step, and condition when reached. A runtime condition error
  ends the debug run with a failure instead of silently skipping the breakpoint.
- **Parent and child frames:** Put a breakpoint on `stepId: inspect-child`,
  start the parent configuration, and Continue from the initial pause. Select
  each Call Stack frame. `$inputs.label` and the Inputs scope must show
  `parent-value` in `parent::call-child` and `child-value` in
  `child::inspect-child`.
- **Malformed source diagnostic:** Launch the malformed configuration or set
  a breakpoint in that file. The message must name the file and YAML error;
  the breakpoint must not appear verified.

The intentionally malformed YAML fixture uses a `.yaml` suffix so it is not
collected as a valid Arazzo document by the repository round-trip suite. This
workspace assigns it the Arazzo language for breakpoint testing.

Protocol regressions also cover nonexistent source files, invalid frame IDs,
expired variable references, and source locations absent from the index.
These cannot all be expressed as Arazzo document contents; the tests send the
corresponding debugger requests using these same fixture documents.
