use std::time::Duration;

use serde_json::Value;

use crate::limits::McpLimits;

pub(crate) struct TimeoutPolicy {
    pub(crate) http_seconds: u64,
    pub(crate) execution_seconds: u64,
}

impl From<&McpLimits> for TimeoutPolicy {
    fn from(limits: &McpLimits) -> Self {
        Self {
            http_seconds: limits.max_http_timeout.as_secs(),
            execution_seconds: limits.max_execution_timeout.as_secs(),
        }
    }
}

impl TimeoutPolicy {
    pub(crate) fn parse(&self, args: &Value) -> Result<(Duration, Duration), String> {
        Ok((
            Self::seconds(
                args.get("http_timeout_seconds"),
                self.http_seconds,
                "http_timeout_seconds",
            )?,
            Self::seconds(
                args.get("execution_timeout_seconds"),
                self.execution_seconds,
                "execution_timeout_seconds",
            )?,
        ))
    }

    fn seconds(value: Option<&Value>, maximum: u64, field: &str) -> Result<Duration, String> {
        let seconds = match value {
            None => maximum,
            Some(value) => value
                .as_u64()
                .filter(|n| (1..=maximum).contains(n))
                .ok_or_else(|| format!("{field} must be an integer from 1 to {maximum}"))?,
        };
        Ok(Duration::from_secs(seconds))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_and_boundaries_reach_exact_durations() {
        let policy = TimeoutPolicy::from(&McpLimits::default());
        for (args, http, execution) in [
            (json!({}), 30, 300),
            (
                json!({"http_timeout_seconds":1,"execution_timeout_seconds":1}),
                1,
                1,
            ),
            (
                json!({"http_timeout_seconds":30,"execution_timeout_seconds":300}),
                30,
                300,
            ),
        ] {
            assert_eq!(
                policy.parse(&args),
                Ok((Duration::from_secs(http), Duration::from_secs(execution)))
            );
        }
    }
}
