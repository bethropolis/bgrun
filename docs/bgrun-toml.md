# bgrun.toml Reference

Place a `bgrun.toml` in your project's git root (or any parent directory containing a `.git` directory). The CLI searches upward from `cwd` to find it.

This file defines named jobs with their commands, readiness checks, dependencies, and restart policies. Named jobs can be referenced from any CLI command instead of typing full command strings.

## Complete example

```toml
[jobs.db]
cmd = "docker run --rm -p 5432:5432 postgres:16"
ready-when-port = 5432
workspace = "myapp"

[jobs.server]
cmd = "cargo run"
ready-when = "listening on"
workspace = "myapp"
after = "db"
restart = "on-crash"

[jobs.worker]
cmd = "python -m celery -A tasks worker"
ready-when = "celery@"
workspace = "myapp"
after = "db"
restart = "on-crash"
```

## Fields

| Key | Type | Description |
|---|---|---|
| `cmd` | string (required) | The shell command to run. Split on whitespace into argv. |
| `ready-when` | string | Mark the job `Ready` when a log line contains this substring. |
| `ready-when-port` | integer | Mark the job `Ready` when `localhost:<port>` accepts TCP connections. |
| `ready-when-url` | string | Mark the job `Ready` when `GET <url>` returns HTTP 2xx. |
| `ready-when-file` | string | Mark the job `Ready` when the given file path exists. |
| `ready-when-regex` | string | Mark the job `Ready` when a log line matches this regex. |
| `ready-when-file` | string | Mark the job `Ready` when the given file path exists. |
| `restart` | string | `"on-crash"` — restart if the process exits with non-zero code or is killed by signal. |
| `workspace` | string | Group jobs for batch operations (`bgrun list --workspace`, `bgrun kill --workspace`). |
| `after` | string | Name of another job that must reach `Ready` (or exit) before this one starts. 120s timeout. |
| `pty` | bool | Allocate a pseudo-terminal for the child process. |
| `max-rss-mb` | integer | Kill the job if its RSS exceeds this value (MB). |
| `max-runtime-ms` | integer | Kill the job after this many milliseconds. |
| `backoff-ms` | integer | Base backoff in ms for restart delay (default: 2000). Doubles each consecutive failure, capped at 5 min. |
| `cwd` | string | Working directory for the job. |
| `env` | table | Environment variables, e.g. `env = { FOO = "bar", BAZ = "qux" }`. |
| `allocate-port` | string | Allocate a free ephemeral TCP port and set it as this env var name (e.g. `"PORT"`). |
| `health-check-url` | string | Poll this HTTP URL periodically after ready for liveness. |
| `health-check-port` | integer | Probe this TCP port periodically after ready for liveness. |
| `health-interval-secs` | integer | Seconds between health checks (default: 10). |
| `health-threshold` | integer | Consecutive failures before killing (default: 3). |
| `cols` | integer | PTY width in columns (default: 80, only with `pty = true`). |
| `rows` | integer | PTY height in rows (default: 24, only with `pty = true`). |

A maximum of one readiness strategy can be configured. If multiple are specified, the CLI picks the first one found in order: `ready-when`, `ready-when-port`, `ready-when-url`, `ready-when-file`.

## Name resolution rules

1. `bgrun run`, `bgrun run-group`, and all commands accepting `--name` check `bgrun.toml` first.
2. If the name matches a `[jobs.<name>]` entry, the configured `cmd`, `workspace`, `after`, and readiness are used as defaults.
3. CLI flags override config values. Example: `bgrun run --name server "cargo run --release"` uses the custom command but still inherits the config's `workspace` and `after`.
4. If a name is not found in `bgrun.toml`, it's treated as a raw command to execute.

## Run-group behavior

`bgrun run-group` spawns all named jobs in parallel but respects `after` dependencies:

```
bgrun run-group db server worker
```

Execution order:

1. `db` starts immediately (no dependencies).
2. `server` waits for `db` to become `Ready` or exit.
3. `worker` waits for `db` to become `Ready` or exit.

`server` and `worker` may start in any order once `db` is resolved, since neither depends on the other.

**Timeout:** If a dependency doesn't reach `Ready` or exit within 120 seconds, the dependent job fails with a timeout error.

## Idempotency

Named jobs are idempotent: running `bgrun run --name server` when a `server` job is already alive returns the existing job record instead of spawning a duplicate. This is safe for scripts and agent loops that call `run` repeatedly.

To force a restart, kill the job first:

```bash
bgrun kill server
bgrun run --name server "cargo run"
```

## Config discovery

The CLI walks from the current working directory up to the git root (first directory containing `.git`) looking for `bgrun.toml`. If found, it's loaded automatically. No `--config` flag needed.

```text
myproject/
├── .git/
├── bgrun.toml       ← found here
├── src/
│   └── main.rs
└── tests/
    └── ...
```
