# MCP security design status

MCP returns to the CLI under the explicit operator-trust model decomposed in
[epic ac-664e5](https://tkt.stevetrefethen.com/docs/ac-tickets/ac-664e5); its
final ticket rewrites this page with what is and is not restricted. The earlier
[MCP security hardening draft](../../plans/current/mcp-security-hardening.md)
is retained as the record of superseded policy and deferred network work; it
distinguishes approved policy from unimplemented mechanisms, so consult
implementation and exact-candidate evidence before asserting protection.

The older March 2026 notes at this path are retired. Local consent does not
make arbitrary file access safe, tool parameters can select paths and trigger
network effects, and the notes' expression example was not an Arazzo authority.

The [agent operability proposal](../../plans/current/agent-operability.md)
requires capability descriptions to disclose actual enforcement and uncertainty.
It adds no permissions, exceptions or different resource policy. A tool
description is not an enforcement boundary; errors and evidence retrieval must
respect the same acquisition, network, redaction and resource owners.
