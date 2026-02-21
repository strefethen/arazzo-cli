# Debugger Protocol v1

This document defines the initial internal protocol contract between the debug adapter backend and editor-facing clients.

## Version Marker

- `arazzo_debug_protocol::INTERNAL_DEBUG_PROTOCOL_VERSION == "v1"`

## Transport

1. Newline-delimited JSON over stdio.
2. One request JSON object per line.
3. One response JSON object per line.
4. Empty lines are ignored.

## Request Envelope

```json
{
  "id": 1,
  "method": "initialize",
  "params": {}
}
```

Fields:

1. `id` (u64): request/response correlation id.
2. `method` (string): command name.
3. `params` (JSON value): method payload.

## Response Envelope

```json
{
  "id": 1,
  "ok": true,
  "result": {}
}
```

Error response:

```json
{
  "id": 1,
  "ok": false,
  "error": {
    "code": "METHOD_NOT_SUPPORTED",
    "message": "unsupported debug method: foo"
  }
}
```

## Supported Methods (PR1)

1. `initialize`
2. `ping`
3. `shutdown`

### initialize

Request params:

```json
{
  "clientName": "vscode",
  "clientVersion": "0.1.0",
  "protocolVersion": "v1"
}
```

Response result:

```json
{
  "protocolVersion": "v1",
  "capabilities": {
    "supportsBreakpoints": false,
    "supportsConditionalBreakpoints": false,
    "supportsStepOver": false,
    "supportsStepIn": false,
    "supportsStepOut": false,
    "supportsPause": false,
    "supportsWatches": false
  }
}
```

### ping

No required params.

Response result:

```json
{
  "protocolVersion": "v1",
  "adapterVersion": "v1"
}
```

### shutdown

No required params.

Response result:

```json
{
  "message": "session closed"
}
```
