# AsyncAPI send payload contract assessment (2026-10-23)

Ticket: [ac-99484](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-99484).

**Finding:** the reviewed specifications do not establish where an author supplies
the concrete message value for the current `channelPath` / `action: send` trial.
`requestBody.payload` is a plausible interpretation, but applying it to that
target requires a clarification. Its omission from the `requestBody` field's
target list proves neither permission nor prohibition. This assessment does not
declare that AsyncAPI sends cannot have authored payloads; it identifies the
missing link needed before replacing the fixture's Rust templates.

## Specification evidence

The normative sources are the vendored **Arazzo 1.1.0** and **AsyncAPI 3.0.0**
documents linked below. Quotes and anchors were checked against those local
copies, whose SHA-256 values match `spec/SHA256SUMS`. Repository implementation
and prior trial results are evidence only of existing behavior.

| Source | Explicit rule and its boundary |
| --- | --- |
| Arazzo [Step.channelPath](../../spec/arazzo/v1.1.0.html#stepChannelPath) and [Step.action](../../spec/arazzo/v1.1.0.html#stepAction) | `channelPath` is “A reference to a Source Description Object combined with a JSON Pointer to reference an event channel.” `action` “Describes the message flow intent.” The latter permits `send` and `receive` for AsyncAPI steps. These identify a target and direction, not an instance value. |
| Arazzo [Step.requestBody](../../spec/arazzo/v1.1.0.html#stepRequestBody) | “The request body to pass to an operation as referenced by operationId or operationPath.” The remaining text discusses HTTP method semantics. It neither names `channelPath` nor explicitly says request bodies are HTTP-only. |
| Arazzo [Request Body Object](../../spec/arazzo/v1.1.0.html#request-body-object), [payload](../../spec/arazzo/v1.1.0.html#requestBodyPayload) and [contentType](../../spec/arazzo/v1.1.0.html#requestBodyContentType) | `payload` is `Any`: “A value representing the request body payload.” Literal values, Runtime Expressions and Selector Objects are allowed; expressions/selectors “MUST be evaluated prior to calling the referenced operation.” `contentType` describes request-content serialization, referring to the targeted operation when omitted. These rules establish how an applicable request body gets its value; they do not independently establish its applicability to a channel send. |
| Arazzo [asynchronous success](../../spec/arazzo/v1.1.0.html#defining-success-for-asynchronous-steps) | The text expressly covers AsyncAPI operations “via operationId or channelPath” and says tools send/receive according to the referenced AsyncAPI description. “For AsyncAPI send operations, the step completes immediately after the message is sent.” This specifies completion, without assigning an Arazzo field as that message's value. “Arazzo does not model broker acknowledgment or delivery confirmation.” |
| AsyncAPI [Message Object](../../spec/asyncapi/v3.0.0.html#message-object) | `payload` is typed as Multi Format Schema Object, Schema Object or Reference Object: “Definition of the message payload.” `contentType` is “The content type to use when encoding/decoding a message's payload.” A schema constrains an instance and a media type governs encoding; neither supplies this execution's instance. |
| AsyncAPI [Channel Object](../../spec/asyncapi/v3.0.0.html#channel-object) and [Operation Object](../../spec/asyncapi/v3.0.0.html#operation-object) | A channel's `messages` map describes permitted messages; each sent message must validate against exactly one. An operation specifies its required `action` and channel reference, and can restrict supported messages using references. These rules constrain message identity/validity, without mapping workflow inputs or earlier outputs into a concrete payload. |
| AsyncAPI [Message Example Object](../../spec/asyncapi/v3.0.0.html#-message-example-object) and [Schema Object](../../spec/asyncapi/v3.0.0.html#schema-object) | An example “represents an example of a Message Object”; its payload must validate against the message schema. Schema `default` describes a predefined property value when none is present. Neither section instructs Arazzo to select an example, instantiate a schema, substitute workflow expressions in it, or generate an outbound message from defaults. |

The other apparent routes do not fill this gap. Arazzo
[Parameter.in](../../spec/arazzo/v1.1.0.html#parameterIn) lists `path`, `query`,
`querystring`, `header`, and `cookie`, with workflow-input mapping for
`workflowId`; it has no message-payload location. The
[Step Object](../../spec/arazzo/v1.1.0.html#step-object) has no standalone
`payload` field. `$message.payload` is a value reference, not an assignment
destination, under the [runtime-expression grammar](../../spec/arazzo/v1.1.0.html#runtime-expressions).
An extension convention would be a separate decision, not a resolution supplied
by these rules.

### Does operationId change the answer?

It strengthens the textual case for `requestBody`: the
[Step Object](../../spec/arazzo/v1.1.0.html#step-object) includes AsyncAPI
operations, [Step.operationId](../../spec/arazzo/v1.1.0.html#stepOperationId)
invokes the referenced operation, the request-body target list includes
`operationId`, and asynchronous success explicitly includes that route.
AsyncAPI's [Operations Object](../../spec/asyncapi/v3.0.0.html#operations-object)
uses the dictionary key as its `operationId`.

**Inference:** for a resolvable AsyncAPI `operationId`, reading the supplied
request-body value as the outgoing message payload is a reasonable composition
of those clauses. The reviewed text still does not explicitly equate the two,
and does not state that `channelPath` inherits that request-body contract.
Consequently this assessment does not promote that reading to a confirmed
`channelPath` authoring rule. Changing target form would also require operation
resolution outside this trial: its resolver supports only `channelPath`. No
operation-resolution design or implementation is proposed here.

## Supported surrounding structure, with the missing placement exposed

These two snippets illustrate one JSON shape, `{"id": integer}`, and one message
type per channel. They establish the surrounding declarations, not a complete
executable send-payload example. The comments deliberately leave both outgoing
values unauthored. There is no proposed `payload`, `requestBody`, or extension
field hidden in the skeleton.

```yaml
# exchange.arazzo.yaml — payload placement remains unresolved
arazzo: 1.1.0
info: {title: JSON exchange, version: '1'}
sourceDescriptions:
  - name: events
    url: events.yaml
    type: asyncapi
workflows:
  - workflowId: exchange
    inputs:
      type: object
      required: [id]
      properties:
        id: {type: integer}
    steps:
      - stepId: send
        channelPath: '{$sourceDescriptions.events.url}#/channels/request'
        action: send
        # Missing: where to author the message using $inputs.id.
      - stepId: receive
        channelPath: '{$sourceDescriptions.events.url}#/channels/reply'
        action: receive
        dependsOn: [send]
        successCriteria:
          - condition: '$message.payload#/id > 0'
        outputs:
          id: '$message.payload#/id'
      - stepId: forward
        channelPath: '{$sourceDescriptions.events.url}#/channels/forward'
        action: send
        dependsOn: [receive]
        # Missing: where to author the message using $steps.receive.outputs.id.
```

```yaml
# events.yaml — illustrative addresses; no endpoint-selection policy
asyncapi: 3.0.0
info: {title: JSON exchange, version: '1'}
channels:
  request:
    address: assessment.request
    messages:
      value: {$ref: '#/components/messages/value'}
  reply:
    address: assessment.reply
    messages:
      value: {$ref: '#/components/messages/value'}
  forward:
    address: assessment.forward
    messages:
      value: {$ref: '#/components/messages/value'}
components:
  messages:
    value:
      contentType: application/json
      payload:
        type: object
        required: [id]
        additionalProperties: false
        properties:
          id: {type: integer}
```

Field-by-field check against the specifications:

- Arazzo version, metadata, nonempty sources/workflows, workflow identifier,
  input schema and ordered steps follow the [root](../../spec/arazzo/v1.1.0.html#arazzo-specification-object),
  [Info](../../spec/arazzo/v1.1.0.html#info-object),
  [Source Description](../../spec/arazzo/v1.1.0.html#source-description-object),
  and [Workflow](../../spec/arazzo/v1.1.0.html#workflow-object) definitions.
  `inputs` describes required input data; it does not provide an input value.
- Each unique step has a source-expression/channel pointer and AsyncAPI action.
  The source URL is a relative reference to the adjacent document, and the
  three channel keys exist there. [Source expression resolution](../../spec/arazzo/v1.1.0.html#source-description-expression-resolution)
  supplies `events.url`; no operation named `url` competes with it.
- The [Step fields](../../spec/arazzo/v1.1.0.html#step-object) permit the
  dependencies and `id` output. The nonempty criterion uses the default
  [simple condition](../../spec/arazzo/v1.1.0.html#simple-conditions), so it needs
  no separately declared selector context. The receive output uses the
  grammar's JSON Pointer form, not dotted payload traversal.
- AsyncAPI version and title/version satisfy its [root](../../spec/asyncapi/v3.0.0.html#asyncapi-object)
  and [Info](../../spec/asyncapi/v3.0.0.html#info-object) requirements. The
  [channels](../../spec/asyncapi/v3.0.0.html#channels-object), literal addresses
  and single-entry [messages](../../spec/asyncapi/v3.0.0.html#messages-object)
  maps refer to one reusable [component](../../spec/asyncapi/v3.0.0.html#components-object)
  through local [references](../../spec/asyncapi/v3.0.0.html#reference-object).
  Its message content type and object schema constrain the JSON value; they do
  not create it. Servers/operations are optional root fields; their omission
  supplies no connection or operation-resolution behavior.

Concrete value trace, **conditional on a future clarified send placement**:
with externally supplied workflow input `{"id":7}`, `$inputs.id` is numeric
`7`, so the intended initial message is `{"id":7}`. Given an independently
received message `{"id":42}`, the criterion succeeds, the receive output is
numeric `42`, and `$steps.receive.outputs.id` yields `42`; the intended forward
is `{"id":42}`. These references are supported by the
[runtime expressions](../../spec/arazzo/v1.1.0.html#runtime-expressions), which
state “Runtime expressions preserve the type of the referenced value.” The
dependencies order the steps. **Neither outbound JSON object is actually
assigned anywhere in this skeleton.** An external responder's behavior is an
assumption, not an effect promised by either specification.

## Current trial evidence and limits

In [nats_transport_trial.rs](../../crates/arazzo-runtime/src/runtime_core/nats_transport_trial.rs),
`execute` chooses Rust templates by resolved channel key: `request` uses
`{"order":"$inputs.order"}` and `forward` uses
`{"id":"$steps.receive.outputs.id"}`. It passes those values to
[`payload.rs::resolve_payload_detailed`](../../crates/arazzo-runtime/src/runtime_core/payload.rs),
which recursively resolves literal containers/expressions and selectors; the
trial rejects resulting diagnostics before sending. Evaluator capability is
not a document-authoring contract. `require_supported_step` rejects both
`requestBody` and `correlationId`.

`prepare_receive` discovers exactly one direct receive step in the selected
workflow and prepares its identity and resolved route before connecting.
[`asyncapi_target.rs::resolve_local_channel`](../../crates/arazzo-runtime/src/runtime_core/asyncapi_target.rs)
projects the local channel, action and authored address; it does not source an
outgoing instance. Endpoint, subscription setup, outbound templates and
responder/observer behavior remain fixture-owned. The skeleton above is not a
replacement fixture or a claim that the current executor sends its illustrated
`{"id":7}` value.

The [trial tests](../../crates/arazzo-runtime/src/runtime_core/nats_transport_trial_tests.rs)
and [prior assessment](arazzo-local-nats-transport-trial.md#selected-workflow-receive-preparation-2026-10-23)
record observed `{"order":7}` and `{"id":42}` traffic, selected-workflow
preparation, and rejection of changed receive identity/route before inbox
consumption. The fixture writes empty `messages` maps; that fact does not
demonstrate the specification's exactly-one-message validation rule for sent
instances. The illustrative source above therefore declares an actual message
schema rather than copying that fixture assumption.

The prior `NATS_TRIAL_BUFFER_EXPIRED` timeout remains unexplained; subsequent
passes do not establish its cause or resolution. This assessment adds no
broker run or new transport proof. Production AsyncAPI execution remains
outside the test-only trial, as recorded in the prior assessment. Correlation,
delivery guarantees, durability, recovery, headers authoring and production
transport design are unchanged and out of scope.

## Clarification gate

Before implementing document-authored payloads for this trial, obtain an
authoritative Arazzo clarification of this exact question:

> For an AsyncAPI step targeted by `channelPath` with `action: send`, is the
> outgoing message instance the evaluated `requestBody.payload`? If so, do the
> existing Request Body Object rules for literals, Runtime Expressions,
> Selector Objects and content type apply, and is this the same contract as an
> AsyncAPI `operationId` send? If not, which standard field and evaluation rule
> supplies that instance?

No payload implementation is justified until that placement is established.
Steve can clear the follow-up gate by obtaining or accepting specification
evidence that answers it; accepting a local extension would require a separate,
explicit product decision and would not establish standards conformance. No
upstream request or implementation ticket was created. The result is a completed
bounded assessment for [ac-eb31c](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-eb31c),
not maintainer approval or implemented payload support.
