#![allow(clippy::unwrap_used)]

use arazzo_spec::ArazzoSpec;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

#[tokio::test(start_paused = true)]
async fn completed_success_and_failure_do_not_expire() {
    let spec: ArazzoSpec = serde_json::from_value(json!({
        "arazzo":"1.1.0", "info":{"title":"timeouts","version":"1"},
        "sourceDescriptions":[{"name":"api","url":"https://example.com","type":"openapi"}], "workflows":[{"workflowId":"ok","steps":[{"stepId":"get","operationPath":"{$sourceDescriptions.api.url}#/paths/~1ok/get"}]}]
    }))
    .unwrap();
    let engine = arazzo_runtime::EngineBuilder::new(spec)
        .openapi_spec(serde_json::to_vec(&serde_json::json!({"openapi":"3.2.0","$self":"https://example.com/","info":{"title":"test","version":"1"},"servers":[{"url":"https://example.com"}],"paths":{"/ok":{"get":{"responses":{"200":{"description":"ok"}}}}}})).unwrap(), None)
            .dry_run(true)
        .build()
        .unwrap();
    for (workflow, success) in [("ok", true), ("missing", false)] {
        let handle = engine.execute_with_timeout(workflow, BTreeMap::new(), Duration::from_secs(5));
        let flag = handle.cancel_token().clone();
        assert_eq!(handle.collect().await.outputs.is_ok(), success);
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(flag.outcome() == arazzo_runtime::ExecutionOutcome::Completed);
    }
}
