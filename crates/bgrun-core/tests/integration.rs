use std::sync::Arc;

use bgrun_core::{Job, JobStore};
use bgrun_proto::{Command, KillArgs, RunArgs, TailArgs};
use tokio::sync::Mutex;

/// Helper to create a test store.
async fn setup() -> Arc<Mutex<JobStore>> {
    Arc::new(Mutex::new(JobStore::new()))
}

#[tokio::test]
async fn test_job_lifecycle() {
    let store = setup().await;

    // Create a job
    let mut job = Job::new(
        "test-1".into(),
        vec!["sleep".into(), "60".into()],
        Some("my-job".into()),
        Some("my-workspace".into()),
    );
    assert_eq!(job.state, bgrun_proto::JobState::Starting);
    assert!(job.is_alive());

    // Transition to Running
    job.transition(bgrun_proto::JobState::Running).unwrap();
    assert_eq!(job.state, bgrun_proto::JobState::Running);

    // Set readiness config
    job.readiness = Some(bgrun_proto::ReadinessStrategy::LogPattern(
        "listening on".into(),
    ));
    job.restart = Some(bgrun_proto::RestartPolicy::OnCrash { backoff_ms: 2000 });
    job.pty = true;
    job.max_runtime_ms = Some(300_000);
    job.env.insert("RUST_LOG".into(), "debug".into());

    // Insert into store
    {
        let mut s = store.lock().await;
        s.insert(job);
    }

    // Verify the job is in the store with all fields
    {
        let s = store.lock().await;
        let job = s.get("test-1").unwrap();
        assert_eq!(job.name, Some("my-job".into()));
        assert_eq!(job.workspace, Some("my-workspace".into()));
        assert!(job.pty);
        assert_eq!(job.max_runtime_ms, Some(300_000));
        assert_eq!(job.env.get("RUST_LOG").unwrap(), "debug");
    }

    // Convert to record and verify serialization preserves all fields
    {
        let s = store.lock().await;
        let job = s.get("test-1").unwrap();
        let record = job.to_record();
        let json = serde_json::to_string(&record).unwrap();
        let parsed: bgrun_proto::JobRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.restart, job.restart);
        assert!(parsed.pty);
        assert_eq!(parsed.max_runtime_ms, Some(300_000));
        assert_eq!(parsed.env.get("RUST_LOG").unwrap(), "debug");
    }

    // Transition to Ready
    {
        let mut s = store.lock().await;
        let job = s.get_mut("test-1").unwrap();
        job.transition(bgrun_proto::JobState::Ready).unwrap();
        job.ready_at = Some(chrono::Utc::now());
    }

    // Verify state
    {
        let s = store.lock().await;
        let job = s.get("test-1").unwrap();
        assert_eq!(job.state, bgrun_proto::JobState::Ready);
        assert!(job.ready_at.is_some());
    }

    // Kill
    {
        let mut s = store.lock().await;
        let job = s.get_mut("test-1").unwrap();
        job.transition(bgrun_proto::JobState::Killed).unwrap();
    }

    {
        let s = store.lock().await;
        let job = s.get("test-1").unwrap();
        assert_eq!(job.state, bgrun_proto::JobState::Killed);
        assert!(!job.is_alive());
    }
}

#[tokio::test]
async fn test_idempotent_named_jobs() {
    let store = setup().await;

    let mut job1 = Job::new(
        "j1".into(),
        vec!["cmd1".into()],
        Some("server".into()),
        None,
    );
    job1.state = bgrun_proto::JobState::Running;

    let mut job2 = Job::new(
        "j2".into(),
        vec!["cmd2".into()],
        Some("server".into()),
        None,
    );
    job2.state = bgrun_proto::JobState::Starting;

    {
        let mut s = store.lock().await;
        s.insert(job1);
        s.insert(job2);
    }

    // find_by_name returns the most recently inserted job
    {
        let s = store.lock().await;
        let job = s.find_by_name("server").unwrap();
        assert_eq!(job.id, "j2");
        assert_eq!(job.cmd, vec!["cmd2".to_string()]);
    }
}

#[tokio::test]
async fn test_proto_serialization_roundtrip() {
    let args = RunArgs {
        cmd: vec!["cargo".into(), "run".into()],
        name: Some("dev-server".into()),
        workspace: Some("myproject".into()),
        readiness: Some(bgrun_proto::ReadinessStrategy::TcpPort(3000)),
        restart: Some(bgrun_proto::RestartPolicy::OnCrash { backoff_ms: 1000 }),
        pty: true,
        max_runtime_ms: Some(600_000),
        max_rss_mb: None,
        env: [("RUST_LOG".into(), "debug".into())].into(),
        after: Some("db".into()),
        cwd: None,
        allocate_port: None,
        health_check: None,
        health_interval_secs: None,
        health_threshold: None,
        pty_cols: None,
        pty_rows: None,
    };

    let json = serde_json::to_string(&args).unwrap();
    let parsed: RunArgs = serde_json::from_str(&json).unwrap();
    assert_eq!(args, parsed);

    // Verify the command tag serializes correctly
    let request = bgrun_proto::Request {
        id: "req-1".into(),
        command: Command::Run(args),
    };
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("\"command\":\"Run\""));
    assert!(json.contains("\"tcp_port\""));
    assert!(json.contains("\"pty\":true"));
}

#[tokio::test]
async fn test_tail_args_serialization() {
    let args = TailArgs {
        id: "job-1".into(),
        lines: 50,
        digest: true,
        level: Some("error".into()),
        strip_ansi: false,
        stream: None,
        cursor: None,
        follow: false,
        filter_regex: None,
    };
    let json = serde_json::to_string(&args).unwrap();
    let parsed: TailArgs = serde_json::from_str(&json).unwrap();
    assert_eq!(args, parsed);
}

#[tokio::test]
async fn test_kill_args_serialization() {
    let args = KillArgs {
        id: Some("job-1".into()),
        workspace: None,
    };
    let json = serde_json::to_string(&args).unwrap();
    let parsed: KillArgs = serde_json::from_str(&json).unwrap();
    assert_eq!(args, parsed);
}

#[tokio::test]
async fn test_config_parsing_and_resolution() {
    let toml_content = r#"
[jobs.server]
cmd = "cargo run --release"
ready-when = "listening on"
workspace = "myproject"
restart = "on-crash"

[jobs.db]
cmd = "docker run --rm -p 5432:5432 postgres:16"
ready-when-port = 5432

[jobs.worker]
cmd = "cargo run --bin worker"
after = "db"
"#;

    let config = bgrun_core::config::parse_config(toml_content).unwrap();
    assert_eq!(config.jobs.len(), 3);

    let args = bgrun_core::config::resolve_job_args("server", &config).unwrap();
    assert_eq!(args.cmd, vec!["cargo", "run", "--release"]);
    assert_eq!(
        args.readiness,
        Some(bgrun_proto::ReadinessStrategy::LogPattern(
            "listening on".into()
        ))
    );
    assert_eq!(args.workspace, Some("myproject".into()));

    let args = bgrun_core::config::resolve_job_args("db", &config).unwrap();
    assert_eq!(
        args.readiness,
        Some(bgrun_proto::ReadinessStrategy::TcpPort(5432))
    );

    let args = bgrun_core::config::resolve_job_args("worker", &config).unwrap();
    assert_eq!(args.after, Some("db".into()));
    assert!(args.readiness.is_none());

    let err = bgrun_core::config::resolve_job_args("nonexistent", &config).unwrap_err();
    assert!(matches!(err, bgrun_core::config::Error::JobNotFound(_)));
}
