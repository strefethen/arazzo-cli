use std::time::Duration;

use serde_json::Value;

#[derive(Default)]
pub(crate) struct TimeoutPolicy;

impl TimeoutPolicy {
    pub(crate) const HTTP_SECONDS: u64 = 30;
    pub(crate) const EXECUTION_SECONDS: u64 = 300;

    pub(crate) fn parse(&self, args: &Value) -> Result<(Duration, Duration), &'static str> {
        Ok((
            Self::seconds(
                args.get("http_timeout_seconds"),
                Self::HTTP_SECONDS,
                "http_timeout_seconds must be an integer from 1 to 30",
            )?,
            Self::seconds(
                args.get("execution_timeout_seconds"),
                Self::EXECUTION_SECONDS,
                "execution_timeout_seconds must be an integer from 1 to 300",
            )?,
        ))
    }

    fn seconds(
        value: Option<&Value>,
        maximum: u64,
        message: &'static str,
    ) -> Result<Duration, &'static str> {
        let seconds = match value {
            None => maximum,
            Some(value) => value
                .as_u64()
                .filter(|n| (1..=maximum).contains(n))
                .ok_or(message)?,
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
        let policy = TimeoutPolicy;
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
