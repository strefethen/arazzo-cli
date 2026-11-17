# MCP security design status

MCP is back in the CLI as `arazzo-cli serve` under the explicit operator-trust
model delivered by
[epic ac-664e5](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-664e5). The
connected client is trusted to execute every loaded workflow with the server
process's credentials; the server restricts file roots, request and result
sizes, timeouts, and failure text, and asks for no approval per operation. The
restricted and not-restricted lists live in
[README: MCP security](../../README.md#security) and
[SECURITY.md](../../SECURITY.md#mcp-trust-model).

Future work, not current protection:

- Network destination policy: allowed hosts, private-network denial, DNS
  rebinding checks, and redirect validation.
- A destination policy for Kafka brokers discovered through cluster metadata.
- A budget for memory held by already-parsed values.
- Containment against a local process changing files during a read.

The [agent operability proposal](../../plans/current/agent-operability.md)
requires capability descriptions to disclose actual enforcement and uncertainty.
It adds no permissions, exceptions or different resource policy. A tool
description is not an enforcement boundary; errors and evidence retrieval must
respect the same acquisition, network, redaction and resource owners.
