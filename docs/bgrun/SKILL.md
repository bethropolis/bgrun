---
name: bgrun
description: >
  Use bgrun whenever you need to run a long-running process (server, build,
  test runner, compiler, database, worker) without blocking your workflow.
  bgrun detaches the process immediately and returns a job ID — you keep working
  while the process runs in the background. Always use bgrun instead of a bare
  shell command for: dev servers, `cargo run`, `npm run dev`, `docker run`,
  any `sleep`-and-poll pattern, or any command that would hang the terminal.
  Use `bgrun wait` to block only until the process is actually ready (not a
  fixed sleep). Use `bgrun tail --digest` to check logs without burning tokens.
  This skill is essential for multi-step agent workflows that involve servers or
  long builds.
---

# bgrun — Background Process Runner

bgrun is a Unix CLI tool that runs commands as detached background jobs managed
by a persistent daemon. The CLI returns immediately. The daemon tracks the
process, captures logs, and monitors readiness. Agents communicate with the
daemon through structured subcommands.

---

## Core Concept

```
bgrun run "cargo run --release"
# → returns JSON immediately with job id, pid, state
# → daemon runs the process, captures stdout+stderr, watches for readiness
# → you do other work
bgrun wait <id> --timeout 30s
# → blocks only until process is ready (or timeout)
# → then test, inspect, kill
```

**Output is auto-detected:** when stdout is a pipe (agent context), all output
is newline-delimited JSON. When stdout is a terminal, output is human-readable.
In agent context you always get JSON — no flags needed.

---

## Commands

### `bgrun run` — Start a background job

```bash
bgrun run [FLAGS] <command...>
```

Returns immediately. The job starts in the daemon.

**Flags:**

| Flag | Type | Purpose |
|------|------|---------|
| `--name <name>` | string | Name for the job (idempotent: returns existing if alive) |
| `--workspace <ws>` | string | Group tag for bulk operations |
| `--ready-when <pattern>` | string | Wait for this substring in stdout/stderr |
| `--ready-when-port <port>` | u16 | Wait for TCP port to accept connections |
| `--ready-when-url <url>` | string | Wait for HTTP endpoint to return 2xx |
| `--ready-when-file <path>` | string | Wait for file to exist |
| `--after <name>` | string | Don't start until named job is ready |
| `--restart on-crash` | string | Auto-restart if process exits non-zero |
| `--backoff <duration>` | string | Delay between restart attempts (e.g. `2s`, `5m`) |
| `--pty` | flag | Allocate a pseudo-terminal (for programs that check for TTY) |

**JSON output shape:**
```json
{
  "id": "b3f7a21c-...",
  "name": "dev-server",
  "workspace": "myproject",
  "cmd": ["cargo", "run", "--release"],
  "pid": 48291,
  "state": "running",
  "started_at": "2026-05-18T10:32:00Z",
  "readiness": { "log_pattern": "listening on" }
}
```

**Examples:**
```bash
bgrun run cargo run --release
bgrun run --name server --workspace proj --ready-when "listening on" cargo run
bgrun run --ready-when-port 5432 docker run --rm -p 5432:5432 postgres:16
bgrun run --after db --name worker cargo run --bin worker
bgrun run --restart on-crash --backoff 5s python worker.py
bgrun run --pty npm run dev
```

---

### `bgrun wait` — Block until ready

```bash
bgrun wait <id> --timeout <duration>
```

Blocks until the job's readiness signal fires or the timeout expires.
Returns immediately when ready. Always set `--ready-when*` on `run` first,
otherwise `wait` polls until timeout.

**JSON output shape:**
```json
{ "ready": true, "elapsed_ms": 4200 }
{ "ready": false, "elapsed_ms": 30000 }
```

**Examples:**
```bash
bgrun wait b3f7a21c --timeout 30s
bgrun wait b3f7a21c --timeout 2m
```

---

### `bgrun status` — Check a job

```bash
bgrun status <id>
```

**JSON output shape:**
```json
{
  "state": "ready",
  "exit_code": null,
  "ready_at": "2026-05-18T10:32:04Z",
  "restart_count": 0,
  "last_diff_cursor": 18432
}
```

States: `starting` → `running` → `ready` | `exited` | `crashed` | `killed`

---

### `bgrun list` — List all jobs

```bash
bgrun list [--workspace <ws>]
```

**JSON output shape** (one record per line):
```json
{"id":"b3f7a21c","name":"server","state":"ready","pid":48291,"cmd":["cargo","run"],...}
{"id":"c4e8b32d","name":"worker","state":"running","pid":48302,"cmd":["cargo","run","--bin","worker"],...}
```

---

### `bgrun kill` — Terminate a job

```bash
bgrun kill <id>
bgrun kill --workspace <ws>
```

Sends SIGTERM to the entire process group, then SIGKILL after 5s if still alive.
Child processes are killed too.

**JSON output shape:**
```json
{ "killed": ["b3f7a21c"] }
{ "killed": ["b3f7a21c", "c4e8b32d"] }
```

---

### `bgrun tail` — Read logs

```bash
bgrun tail <id> [--lines <n>] [--digest] [--level <error|warn>]
```

**Raw lines** (default, 20 lines):
```json
{
  "lines": [
    { "line_number": 142, "content": "listening on :8080", "timestamp": null },
    { "line_number": 143, "content": "ready to serve", "timestamp": null }
  ]
}
```

**Digest** (`--digest`) — use this to save tokens:
```json
{
  "total_lines": 4821,
  "errors": 3,
  "warnings": 12,
  "last_error": "thread 'main' panicked at 'connection refused'",
  "last_error_line": 4799
}
```

**Examples:**
```bash
bgrun tail b3f7a21c --lines 10
bgrun tail b3f7a21c --digest
bgrun tail b3f7a21c --level error
bgrun tail b3f7a21c --lines 50 --level warn
```

---

### `bgrun diff` — New lines since last call

```bash
bgrun diff <id>
```

Returns only lines written since the last `bgrun diff` call. The cursor is
persisted by the daemon. Safe to call repeatedly — you only pay for new output.

**JSON output shape:**
```json
{
  "lines": [
    { "line_number": 201, "content": "request received", "timestamp": null }
  ],
  "cursor": 21480
}
```

---

### `bgrun send` — Write to stdin

```bash
bgrun send <id> "<data>"
```

Delivers data to the job's stdin. Useful for interactive programs that prompt
for input.

**Important:** `bgrun send` does **not** add a trailing newline. For line-buffered
programs (e.g. interactive shells), include `\n` explicitly:

```bash
bgrun send b3f7a21c "yes\n"
bgrun send b3f7a21c $'q\n'
```

Works with both piped and `--pty` jobs.

**JSON output shape:**
```json
{ "ok": true }
```

---

### `bgrun stats` — Resource usage

```bash
bgrun stats <id>
```

**JSON output shape:**
```json
{ "cpu_pct": 12.4, "rss_mb": 148, "uptime_secs": 320 }
```

---

### `bgrun run-group` — Start multiple jobs in parallel

```bash
bgrun run-group <name1> <name2> ...
```

Starts multiple named jobs (from `bgrun.toml`) in parallel. Returns all records.

```bash
bgrun run-group server db worker
```

---

## Readiness Signal Guide

Choose the right `--ready-when*` for your process. If you don't know which,
**use `--ready-when`** with a substring you expect to appear in the output.

| Situation | Flag to use | Example |
|-----------|-------------|---------|
| Server prints what port it binds | `--ready-when` | `--ready-when "listening on"` |
| Known port, no predictable log output | `--ready-when-port` | `--ready-when-port 3000` |
| Server has a health endpoint | `--ready-when-url` | `--ready-when-url http://localhost:8080/health` |
| Process writes a PID file or lock file | `--ready-when-file` | `--ready-when-file ./tmp/server.pid` |
| No readiness signal at all | none + fixed `bgrun wait --timeout` | wait with generous timeout |

**`--ready-when` pattern tips:**
- Match what the process actually prints, e.g. `"Server started"`, `"ready"`, `"listening"`, `"bound to"`
- Partial match is fine — `"listening on"` matches `"listening on :8080"` or `"listening on 0.0.0.0:3000"`
- Case-sensitive. Match the casing the process uses.
- If unsure what text a process prints, run it once manually or use `bgrun tail` to inspect

---

## Token-Efficient Log Reading

Prefer `--digest` when you just want to know if something went wrong. Use
`--lines` only when you need to read actual content. Use `--diff` in polling
loops to avoid re-reading old output.

```bash
# Cheapest: just counts — good for "did anything fail?"
bgrun tail <id> --digest

# Medium: filtered lines only — good for "what errors occurred?"
bgrun tail <id> --level error --lines 20

# Polling loop: only new content each time
bgrun diff <id>   # call repeatedly; only new lines each call

# Expensive: avoid unless you need full raw output
bgrun tail <id> --lines 200
```

---

## `bgrun.toml` — Project Config

If a `bgrun.toml` exists in the project root (or any parent directory up to the
git root), named jobs are defined there and you can reference them by name.

```toml
[jobs.server]
cmd = "cargo run --release"
ready-when = "listening on"
workspace = "myproject"

[jobs.db]
cmd = "docker run --rm -p 5432:5432 -e POSTGRES_PASSWORD=dev postgres:16"
ready-when-port = 5432
workspace = "myproject"

[jobs.worker]
cmd = "cargo run --bin worker"
after = "db"
workspace = "myproject"
```

With this config:
```bash
bgrun run server          # resolves cmd + readiness from toml
bgrun run server          # idempotent — returns existing job if alive
bgrun run-group server db worker
bgrun kill --workspace myproject
```

Command-line flags override toml values when both are present.

---

## Common Agent Workflows

### Start a server and test it

```bash
# 1. Start server in background with readiness signal
bgrun run --name dev --ready-when "listening on" cargo run

# 2. Block until ready (not a sleep!)
bgrun wait <id> --timeout 60s
# → { "ready": true, "elapsed_ms": 4200 }

# 3. Run your tests against it
cargo test --test integration

# 4. Tear down
bgrun kill <id>
```

---

### Start a multi-service stack

```bash
# Start db first, then server (which depends on db)
bgrun run --name db --ready-when-port 5432 docker run --rm -p 5432:5432 postgres:16
bgrun run --name server --after db --ready-when "listening on" cargo run

# or, with bgrun.toml defining both:
bgrun run-group db server

# Wait for everything
bgrun wait <server-id> --timeout 60s

# Tear down the whole stack at once
bgrun kill --workspace myproject
```

---

### Build and check output

```bash
# Long build in background
bgrun run --name build cargo build --workspace

# Poll status rather than blocking
bgrun status <id>
# when state == "exited" (exit_code 0) or "crashed" (exit_code != 0), done

# Check for errors without reading all output
bgrun tail <id> --digest
# → { "errors": 2, "warnings": 5, "last_error": "error[E0308]: ..." }
```

---

### Watch a server's logs during a test run

```bash
# Start the server
bgrun run --name srv --ready-when-port 8080 ./myserver
bgrun wait <srv-id> --timeout 30s

# Start the test runner in background too
bgrun run --name tests cargo test --test e2e

# Poll for new server logs while tests run
bgrun diff <srv-id>     # call this each iteration to see new output only

# When tests finish, check results
bgrun status <tests-id>
bgrun tail <tests-id> --level error --lines 30
```

---

### Restart policy for flaky servers

```bash
bgrun run --name flaky-server --restart on-crash --backoff 2s ./server
# Check restart_count in status to see if it's been crashing
bgrun status <id>
bgrun stats <id>
```

---

## Error Handling

All commands return `{ "ok": false, "error": "message" }` on failure.

**Common errors and causes:**

| Error message | Cause | Fix |
|---|---|---|
| `daemon failed to start within 1 second` | daemon binary not in PATH or wrong location | ensure `bgrun-daemon` binary is next to `bgrun` binary |
| `job not found` | stale ID, daemon restarted | `bgrun list` to see current jobs |
| `port 3000 already in use` | port conflict pre-check fired | kill whatever is on that port first |
| `dependency 'db' not found` | `--after db` but no job named `db` is running | start the dependency first |
| `command must not be empty` | empty cmd arg | check cmd parsing |
| `no stdin handle for job` | job was not started with stdin piped, or handle consumed | send only works on interactive processes |

---

## Anti-Patterns

**Don't do this:**
```bash
# ❌ blocking — hangs the agent
cargo run &
sleep 10
curl http://localhost:3000

# ❌ re-reading full logs every tick
bgrun tail <id> --lines 1000   # in a loop

# ❌ polling with fixed sleep instead of wait
bgrun run <cmd>; sleep 5; bgrun status <id>  # fragile

# ❌ killing by PID — misses child processes
kill $(bgrun status <id> | jq .pid)
```

**Do this instead:**
```bash
# ✅ non-blocking with proper readiness
bgrun run --ready-when "ready" cargo run
bgrun wait <id> --timeout 30s

# ✅ token-efficient log reading
bgrun tail <id> --digest         # just counts
bgrun diff <id>                  # only new lines

# ✅ kill the full process group
bgrun kill <id>                  # kills entire process tree
bgrun kill --workspace myproject # kills all related jobs
```

---

## State Reference

| State | Meaning | `is_alive` |
|-------|---------|------------|
| `starting` | Spawned, not yet running | yes |
| `running` | Running, no readiness configured or not yet ready | yes |
| `ready` | Readiness signal fired | yes |
| `exited` | Exited with code 0 | no |
| `crashed` | Exited with non-zero code | no |
| `killed` | Killed by `bgrun kill` | no |

A job that is `exited` or `crashed` will not become alive again unless you
`bgrun run` it again (restart policy handles this automatically if configured).

---

## State on Disk

bgrun persists all job state across daemon restarts:

```
~/.local/share/bgrun/
├── daemon.log          # daemon's own log (BGRUN_LOG=debug for verbose)
├── daemon.pid
├── audit.log           # append-only record of every invocation
└── jobs/{job-id}/
    ├── meta.json       # command, name, readiness config
    ├── status.json     # state, exit_code, ready_at, diff cursor
    └── stdout.log      # merged stdout+stderr (rotates at 50MB)
```

To debug the daemon itself:
```bash
BGRUN_LOG=debug bgrun run ...
cat ~/.local/share/bgrun/daemon.log
```
