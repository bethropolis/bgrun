use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The state of a managed job process.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Starting,
    Running,
    Ready,
    Exited,
    Crashed,
    Killed,
}

impl std::fmt::Display for JobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobState::Starting => write!(f, "starting"),
            JobState::Running => write!(f, "running"),
            JobState::Ready => write!(f, "ready"),
            JobState::Exited => write!(f, "exited"),
            JobState::Crashed => write!(f, "crashed"),
            JobState::Killed => write!(f, "killed"),
        }
    }
}

/// Strategy for detecting when a job is ready to serve traffic.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStrategy {
    LogPattern(String),
    #[serde(rename = "log_pattern_regex")]
    LogPatternRegex(String),
    HttpPoll(String),
    TcpPort(u16),
    FileExists(String),
}

/// Policy for restarting a job after it exits.
///
/// - `Never`: never restart, even on crash.
/// - `OnCrash`: restart only when the job lands in `Crashed` (non-zero exit
///   or signal death). Clean exits are left alone.
/// - `OnFailure`: like `OnCrash`, plus restart when the job exits cleanly
///   without ever becoming `Ready` (a readiness gate was configured but the
///   job died before satisfying it — treated as a failed start).
/// - `Always`: restart on any terminal state except `Killed` (an explicit
///   user kill is always respected).
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub enum RestartPolicy {
    Never,
    OnCrash { backoff_ms: u64 },
    OnFailure { backoff_ms: u64 },
    Always { backoff_ms: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_strategy_roundtrips() {
        let strategy = ReadinessStrategy::TcpPort(3000);
        let json = serde_json::to_string(&strategy).expect("strategy should serialize");
        let parsed: ReadinessStrategy =
            serde_json::from_str(&json).expect("strategy should deserialize");
        assert_eq!(strategy, parsed);
    }

    #[test]
    fn restart_policy_roundtrips() {
        for policy in [
            RestartPolicy::Never,
            RestartPolicy::OnCrash { backoff_ms: 1000 },
            RestartPolicy::OnFailure { backoff_ms: 2000 },
            RestartPolicy::Always { backoff_ms: 500 },
        ] {
            let json = serde_json::to_string(&policy).expect("policy should serialize");
            let parsed: RestartPolicy =
                serde_json::from_str(&json).expect("policy should deserialize");
            assert_eq!(policy, parsed);
        }
        // Pre-existing persisted form (PascalCase, no rename_all) still parses.
        let legacy: RestartPolicy = serde_json::from_str(r#"{"OnCrash":{"backoff_ms":2000}}"#)
            .expect("legacy OnCrash should deserialize");
        assert_eq!(legacy, RestartPolicy::OnCrash { backoff_ms: 2000 });
    }

    #[test]
    fn test_readiness_log_pattern_regex_roundtrip() {
        let strategy = ReadinessStrategy::LogPatternRegex(r"^Hello \d+$".into());
        let json = serde_json::to_string(&strategy).unwrap();
        assert!(json.contains("log_pattern_regex"));
        let parsed: ReadinessStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(strategy, parsed);
    }
}
