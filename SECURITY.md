# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability or suspect a security boundary can be bypassed, report it privately using [GitHub's vulnerability reporting](https://github.com/strefethen/arazzo-cli/security/advisories/new):

1. Go to the [Security tab](https://github.com/strefethen/arazzo-cli/security)
2. Click **Report a vulnerability**
3. Provide affected versions or commits, platform, relevant configuration, expected and observed behavior, security impact, and minimal steps to reproduce

Use synthetic data and redact credentials, personal data, file contents, internal paths, and private service addresses from reports and attachments. Include only the minimum information needed to reproduce the issue. Test only systems and data you are authorized to access; do not retrieve real secrets or perform destructive tests to demonstrate a finding.

Please do not put vulnerability details in public issues, discussions, or pull requests. If the private reporting option is unavailable, open an issue requesting a private security contact without including vulnerability details. Coordinate public disclosure with the maintainer so users can receive actionable mitigation or a fix. See GitHub's guidance on [private reporting](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/report-privately) and [coordinated disclosure](https://docs.github.com/en/code-security/concepts/vulnerability-reporting-and-management/coordinated-disclosure).

## Response

I'll acknowledge reports within a reasonable timeframe and provide an update when a fix is available. There is no formal SLA — this is a single-maintainer project.

## Scope

This policy covers this repository's CLI, workspace crates, MCP server, and VS Code debug adapter, including dependency vulnerabilities that affect their security. Report upstream dependency defects to their maintainers as well; let this project know privately when its use of the dependency creates exposure.

## MCP trust model

`arazzo-cli serve` and `arazzo-mcp` trust the connected MCP client. It can execute every loaded workflow, including its API side effects, with the server process's credentials and environment, and the server asks for no approval per operation. Both print a fixed notice saying so on stderr at startup.

Restricted:

- File reads by tools and referenced documents, to the configured roots, on every platform: lexical `..` escapes, dot-prefixed names, non-regular files, and paths that resolve outside a root are denied
- Inbound request frame size and the pending-request backlog
- Tool result size and HTTP response body size
- Document size
- HTTP and execution timeout ceilings
- Cancellation of in-flight runs when the client disconnects or cancels
- Credential redaction in failure text

Not restricted:

- Which hosts a workflow may contact, including localhost and private networks
- DNS rebinding
- Broker connections discovered through Kafka metadata
- Memory used by already-parsed values
- A local process changing files while they are being read
- Anything a connected client does with the capabilities the operator granted

A report that the server crosses a restricted boundary is a vulnerability. A report about an item in the second list is a known limitation, not a vulnerability.

## MCP deployment risks

MCP allows an AI client to request file reads and API workflow execution. The server runs with the permissions of the account that launches it; stdio does not provide isolation, and the file roots are a path restriction, not a sandbox. Untrusted content or prompt injection can cause an agent to request unintended operations. API workflows can modify or delete data and transmit credentials or other sensitive information. Redaction is best-effort; outputs, logs, and traces may contain sensitive data.

Use process isolation, minimal read-only filesystem access, narrowly scoped credentials, external network controls, and MCP-client approval for consequential operations. Review workflows before use, prefer test accounts and non-production data, and keep the application and dependencies updated. These precautions reduce exposure but cannot eliminate risk. See [README: MCP security](README.md#security) and the [official MCP local-server security guidance](https://modelcontextprotocol.io/docs/tutorials/security/local-server-security).

## Assumption of risk and warranty

You are responsible for deciding whether MCP is appropriate for your environment and for the permissions and operations you authorize. By enabling the feature, you assume the risks associated with its use, including unintended file disclosure, credential exposure, and API changes or data loss. The software is provided as-is without warranty under the [MIT License](LICENSE); that license governs its warranty disclaimer and limitation of liability. This notice does not replace the license, override applicable law, or change the vulnerability-reporting process above.
