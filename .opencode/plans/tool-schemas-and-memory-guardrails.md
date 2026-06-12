# Phase 1: Self-Describing AI Tool Schemas

## 1. Root Cargo.toml
Add `schemars` to `[workspace.dependencies]`:
```
schemars = "0.8"
```

## 2. crates/bgrun-proto/Cargo.toml
Add to `[dependencies]`:
```
schemars = { workspace = true }
```

## 3. crates/bgrun-proto/src/types.rs
- Add `use schemars::JsonSchema;`
- Derive `JsonSchema` on `ReadinessStrategy` and `RestartPolicy`

## 4. crates/bgrun-proto/src/command.rs
- Add `use schemars::JsonSchema;`
- Derive `JsonSchema` on `RunArgs`, `KillArgs`, `TailArgs`, `Command`

## 5. crates/bgrun-cli/src/commands/schema.rs (new)
```rust
use anyhow::Result;

pub fn print_schema(command: &str) -> Result<()> {
    let schema = match command {
        "run" => schemars::schema_for!(bgrun_proto::RunArgs),
        "kill" => schemars::schema_for!(bgrun_proto::KillArgs),
        "tail" => schemars::schema_for!(bgrun_proto::TailArgs),
        _ => anyhow::bail!("unknown command: {command}"),
    };
    println!("{}", serde_json::to_string_pretty(&schema)?);
    Ok(())
}
```

## 6. crates/bgrun-cli/src/commands/mod.rs
Add `pub mod schema;`

## 7. crates/bgrun-cli/src/main.rs
Add `Schema` variant to `Commands`:
```rust
/// Print JSON Schema for a command's arguments
Schema {
    /// Command name (run, kill, tail)
    command: String,
}
```
Add dispatch arm:
```rust
Commands::Schema { command } => {
    commands::schema::print_schema(&command)?;
}
```

---

# Phase 2: Memory RSS Guardrails

## 8. crates/bgrun-proto/src/command.rs
Add field to `RunArgs`:
```rust
#[serde(skip_serializing_if = "Option::is_none", default)]
pub max_rss_mb: Option<u64>,
```

## 9. crates/bgrun-proto/src/response.rs
Add field to `JobRecord`:
```rust
#[serde(skip_serializing_if = "Option::is_none", default)]
pub max_rss_mb: Option<u64>,
```

## 10. crates/bgrun-core/src/job.rs
- Add field: `pub max_rss_mb: Option<u64>`
- In `new()`: `max_rss_mb: None`
- In `to_record()`: `max_rss_mb: self.max_rss_mb`

## 11. crates/bgrun-daemon/src/state.rs
In `read_all_jobs()`, after `job.env = record.env;`:
```rust
job.max_rss_mb = record.max_rss_mb;
```

## 12. crates/bgrun-cli/src/main.rs
Add `--max-rss` to `Commands::Run`:
```rust
/// Max RSS in MB before job is killed
#[arg(long)]
max_rss: Option<u64>,
```
Add to destructure and pass to `RunFlags`:
```rust
max_rss_mb: max_rss,
```

## 13. crates/bgrun-cli/src/commands/run.rs
Add field to `RunFlags`:
```rust
pub max_rss_mb: Option<u64>,
```
Set it in `RunArgs`:
```rust
max_rss_mb: flags.max_rss_mb,
```

## 14. crates/bgrun-daemon/src/runner.rs
### Add global `SYSINFO_SYSTEM`
```rust
use once_cell::sync::Lazy;
use std::sync::Mutex;

static SYSINFO_SYSTEM: Lazy<Arc<Mutex<sysinfo::System>>> = Lazy::new(|| {
    Arc::new(Mutex::new(sysinfo::System::new()))
});
```

### Add `monitor_memory_limit` function
```rust
async fn monitor_memory_limit(
    id: String,
    max_rss_mb: u64,
    store: SharedStore,
) {
    use tokio::time::{sleep, Duration};
    loop {
        sleep(Duration::from_secs(1)).await;
        let pid = {
            let store_ref = store.lock().await;
            store_ref.get(&id).and_then(|j| j.pid)
        };
        let Some(pid) = pid else { break };
        let rss_kb = get_process_rss_kb(pid);
        if let Some(rss_kb) = rss_kb {
            let rss_mb = rss_kb / 1024;
            if rss_mb > max_rss_mb {
                tracing::warn!(id = %id, rss_mb, max_rss_mb, "memory limit exceeded, killing job");
                let _ = kill_job(&id, store.clone()).await;
                break;
            }
        }
    }
}
```

### Add `get_process_rss_kb` helper
```rust
fn get_process_rss_kb(pid: u32) -> Option<u64> {
    let mut system = SYSINFO_SYSTEM.lock().unwrap();
    system.refresh_process(sysinfo::Pid::from_u32(pid));
    system.process(sysinfo::Pid::from_u32(pid))
        .map(|p| p.memory() as u64)
}
```

### Refactor `get_stats` to use global
Remove `sys` parameter; use `SYSINFO_SYSTEM` internally.

### Spawn monitor in `spawn_job` and `spawn_pty_job`
After the `max_runtime_ms` spawn block:
```rust
if let Some(max_rss_mb) = args.max_rss_mb {
    let store_clone = store.clone();
    let id_clone = id.clone();
    tokio::spawn(async move {
        monitor_memory_limit(id_clone, max_rss_mb, store_clone).await;
    });
}
```

## 15. crates/bgrun-daemon/src/server.rs
- Remove `type SharedSystem = Arc<Mutex<sysinfo::System>>;`
- Remove `sysinfo_system` parameter from `run_server`, `handle_connection`, `dispatch`
- Update `runner::get_stats` call to remove `sysinfo_system` arg
- Remove `sysinfo_system.clone()` in the accept loop

## 16. crates/bgrun-daemon/src/main.rs
- Remove `sysinfo_system` creation (line 47: `let sysinfo_system = ...`)
- Update `server::run_server(socket_path, store).await` — drop the third argument

---

# Verification

```bash
cargo build --workspace
cargo test --workspace
```
