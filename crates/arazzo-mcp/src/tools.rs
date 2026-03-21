//! MCP tool definitions returned by `tools/list`.

use serde_json::{json, Value};

use crate::state::ServerState;

/// Returns the tool definitions for the MCP `tools/list` response.
pub fn definitions(_state: &ServerState) -> Vec<Value> {
    vec![
        json!({
            "name": "list_workflows",
            "description": "List all available Arazzo workflows across loaded specs. Returns workflow IDs, summaries, input names, and output names.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "describe_workflow",
            "description": "Get detailed information about a specific workflow including its full input schema, output descriptions, step summaries, and source descriptions. Use this before run_workflow to discover required inputs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workflow_id": {
                        "type": "string",
                        "description": "The workflow ID to describe"
                    }
                },
                "required": ["workflow_id"]
            }
        }),
        json!({
            "name": "run_workflow",
            "description": "Execute an Arazzo workflow with the given inputs and return its outputs. Use list_workflows or describe_workflow first to discover available workflows and their required inputs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workflow_id": {
                        "type": "string",
                        "description": "The workflow ID to execute"
                    },
                    "inputs": {
                        "type": "object",
                        "description": "Workflow input values as key-value pairs"
                    },
                    "dry_run": {
                        "type": "boolean",
                        "description": "If true, resolve requests without sending them"
                    },
                    "parallel": {
                        "type": "boolean",
                        "description": "Execute independent steps in parallel"
                    },
                    "http_timeout_seconds": {
                        "type": "integer",
                        "description": "Per-request HTTP timeout in seconds (default: 30)"
                    },
                    "execution_timeout_seconds": {
                        "type": "integer",
                        "description": "Overall workflow execution timeout in seconds (default: 300)"
                    }
                },
                "required": ["workflow_id"]
            }
        }),
        json!({
            "name": "validate_spec",
            "description": "Validate an Arazzo spec YAML file. Returns whether the spec is valid and any validation errors found.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the Arazzo spec YAML file to validate"
                    }
                },
                "required": ["file_path"]
            }
        }),
    ]
}
