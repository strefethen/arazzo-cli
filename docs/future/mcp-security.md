# Security Notes
March 20, 2026

- validate_spec filesystem access: This tool accepts arbitrary file paths. Since MCP servers run locally with user consent, this is acceptable for v1. Error messages from
parse failures may reveal file existence or partial content. Future hardening: add an optional --allowed-dirs flag to restrict file access.
- run_workflow network access: Workflow execution makes real HTTP requests to URLs defined in user-provided specs. The agent cannot inject arbitrary URLs — it can only
trigger execution of workflows the user explicitly loaded at startup. This is by design.
- Environment variable exposure: Specs using $env.VAR_NAME expressions can read host environment variables. Sensitive values could leak in workflow outputs returned to the
agent.