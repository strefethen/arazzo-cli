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

## MCP deployment risks

MCP allows an AI client to request file reads and API workflow execution. The server runs with the permissions of the account that launches it; stdio does not provide isolation. Untrusted content or prompt injection can cause an agent to request unintended operations. API workflows can modify or delete data and transmit credentials or other sensitive information.

Current limitations:

- Without `--allowed-dir`, file-accepting tools can read any path available to the process. Dot-prefixed files are not excluded.
- Directory checks happen before file opens and do not guarantee containment if a symlink changes between the check and the read. Parser errors may include file contents and internal paths.
- MCP execution does not currently enforce public-only network destinations or validate DNS results and redirects against a destination policy. Local and private services reachable by the process may be contacted.
- Redaction is best-effort; errors, outputs, logs, and traces may contain sensitive data.

Stronger defaults and enforcement are planned. Do not rely on those protections until they are implemented and documented for the version you run. See [README: MCP security](README.md#security) for current configuration guidance.

Use process isolation, minimal read-only filesystem access, narrowly scoped credentials, external network controls, and MCP-client approval for consequential operations. Review workflows before use, prefer test accounts and non-production data, and keep the application and dependencies updated. These precautions reduce exposure but cannot eliminate risk. See the [official MCP local-server security guidance](https://modelcontextprotocol.io/docs/tutorials/security/local-server-security).

## Assumption of risk and warranty

You are responsible for deciding whether MCP is appropriate for your environment and for the permissions and operations you authorize. By enabling the feature, you assume the risks associated with its use, including unintended file disclosure, credential exposure, and API changes or data loss. The software is provided as-is without warranty under the [MIT License](LICENSE); that license governs its warranty disclaimer and limitation of liability. This notice does not replace the license, override applicable law, or change the vulnerability-reporting process above.
