# Command Reference

bgrun has 11 subcommands. All commands return JSON to stdout when piped, or human-readable output when connected to a terminal.

The daemon auto-starts on the first CLI invocation. You don't need to manually start it.

---

## run

Start a background process.

```
bgrun run [OPTIONS] <cmd> [args...]
```

**Flags**

| Flag | Description |
|---|---|
| `--name <NAME>` | Named job (enables idempotent re-run; second `run --name X` returns the existing job if alive) |
| `--workspace <WS>` | Group jobs for batch operations |
| `--ready-when <PATTERN>` | Mark job `Ready` when a log line matches this substring |
| `--ready-when-port <PORT>` | Mark job `Ready` when TCP port `localhost:PORT` accepts connections |
| `--ready-when-url <URL>` | Mark job `Ready` when GET returns HTTP 2xx |
| `--ready-when-file <PATH>` | Mark job `Ready` when file exists |
| `--after <NAME>` | Wait for named job to reach `Ready` (or `Exited`/`Crashed`) before spawning |
| `--pty` | Allocate a pseudo-terminal (useful for processes that buffer output differently with pipes) |
| `--restart on-crash` | Auto-restart if the process exits non-zero (SIGKILL, crash, non-zero exit) |
| `--backoff <DURATION>` | Delay between restart attempts, e.g. `2s`, `5m`, `500ms` (default: `2s`, only with `--restart`) |

**Examples**

```bash
# Simple background process
bgrun run "npm run dev"

# Named + readiness
bgrun run --name server --ready-when "listening on" "cargo run"

# With restart
bgrun run --name worker --restart on-crash --backoff 5s "python worker.py"

# Depends on another job
bgrun run --name tests --after server "cargo test"

# PTY allocation
bgrun run --pty "npm run dev"
```

**Output** (JSON)

```json
{"id":"abc123","name":"server","workspace":null,"cmd":["cargo","run"],"pid":12345,"state":"running","started_at":"2026-05-31T00:00:00Z"}
```

If a named job is already running, the existing record is returned instead of spawning a duplicate.

---

## run-group

Start multiple named jobs in parallel. Each name is resolved from `bgrun.toml` (see [bgrun.toml Reference](bgrun-toml.md)). Jobs respect their `after` dependencies — the group waits for each job's dependencies before spawning it.

```
bgrun run-group <NAME> [NAME...]
```

**Examples**

```bash
# Start all jobs defined in bgrun.toml
bgrun run-group db server worker

# Start specific group
bgrun run-group server worker
```

**Output** (JSON)

```json
[
  {"id":"aaa","name":"server","state":"running",...},
  {"id":"bbb","name":"worker","state":"running",...}
]
```

---

## list

List all known jobs, or filter by workspace.

```
bgrun list [--workspace <WS>]
```

**Examples**

```bash
# All jobs
bgrun list

# Jobs in a specific workspace
bgrun list --workspace myapp
```

**Output** (JSON) — one JSON object per line (NDJSON):

```json
{"id":"abc","name":"server","workspace":"myapp","cmd":["cargo","run"],"pid":12345,"state":"running","started_at":"..."}
{"id":"def","name":"worker","workspace":"myapp","cmd":["python","worker.py"],"pid":12346,"state":"exited","started_at":"..."}
```

---

## status

Get the current state of a job.

```
bgrun status <ID>
```

**Examples**

```bash
bgrun status abc123
bgrun status server  # works with names too
```

**Output** (JSON)

```json
{"state":"running","exit_code":null,"ready_at":null,"restart_count":0,"last_diff_cursor":0}
```

| Field | Meaning |
|---|---|
| `state` | `Starting`, `Running`, `Ready`, `Exited`, `Crashed`, or `Killed` |
| `exit_code` | Process exit code (null while running) |
| `ready_at` | RFC 3339 timestamp when readiness check passed |
| `restart_count` | How many times the process has been restarted |
| `last_diff_cursor` | Byte offset for incremental log tailing |

---

## tail

Show the last N lines of a job's stdout/stderr log.

```
bgrun tail <ID> [--lines <N>] [--digest] [--level <LEVEL>]
```

**Flags**

| Flag | Description |
|---|---|
| `--lines <N>` | Number of lines to show (default: 20) |
| `--digest` | Show summary instead of raw lines (error/warn count, last error) |
| `--level <LEVEL>` | Filter lines containing `error` or `warn` (case-insensitive) |

**Examples**

```bash
# Last 10 lines
bgrun tail server --lines 10

# Digest summary
bgrun tail server --digest

# Show only lines containing "error"
bgrun tail server --level error
```

**Output** (JSON)

```json
{
  "lines": [
    {"line_number": 42, "content": "listening on :8080", "timestamp": null},
    {"line_number": 43, "content": "GET / 200 OK", "timestamp": null}
  ]
}
```

Human output colorizes errors in red and warnings in yellow.

---

## diff

Show log lines added since the last `diff` call (tracked via cursor).

```
bgrun diff <ID>
```

**Examples**

```bash
# First call: all log content
bgrun diff server

# Later: only new lines since last call
bgrun diff server
```

**Output** (JSON)

```json
{
  "cursor": 2048,
  "lines": [
    {"line_number": 100, "content": "some new output", "timestamp": null}
  ]
}
```

---

## wait

Block until a job reaches `Ready` state, or a timeout elapses.

```
bgrun wait <ID> [--timeout <DURATION>]
```

**Examples**

```bash
# Wait up to 60s
bgrun wait server

# Wait up to 5 minutes
bgrun wait db --timeout 5m
```

**Output** (JSON)

```json
{"ready":true,"elapsed_ms":1234}
```

If the job exits with a non-zero code before becoming Ready, the response is:

```json
{"ready":false,"elapsed_ms":60000}
```

---

## kill

Terminate a job by ID, name, or entire workspace.

```
bgrun kill <ID>
bgrun kill --workspace <WS>
```

Sends `SIGTERM` first, then `SIGKILL` after 5 seconds if the process hasn't exited. Sends to the entire process group, so child processes are cleaned up.

**Examples**

```bash
# Kill by ID
bgrun kill abc123

# Kill by name
bgrun kill server

# Kill all jobs in a workspace
bgrun kill --workspace myapp
```

**Output** (JSON) — single kill:

```json
{"killed":["abc123"]}
```

Workspace kill:

```json
{"killed":["abc123","def456"]}
```

---

## send

Write data to a job's stdin.

```
bgrun send <ID> <DATA>
```

**Examples**

```bash
# Send text
bgrun send server "/reload"

# Quote if data contains spaces
bgrun send server "some multi-word input"
```

**Output** (JSON)

```json
{"ok":true}
```

Only works for jobs that were spawned with piped stdin (the default). Jobs spawned with `--pty` use the PTY for stdin and don't support send.

---

## stats

Show CPU and memory usage of a running process.

```
bgrun stats <ID>
```

**Examples**

```bash
bgrun stats server
```

**Output** (JSON)

```json
{"cpu_pct":2.4,"rss_mb":48,"uptime_secs":3600}
```

| Field | Meaning |
|---|---|
| `cpu_pct` | CPU usage percentage (across all cores) |
| `rss_mb` | Resident memory in MB |
| `uptime_secs` | Process uptime in seconds |

---

## Job States

Jobs transition through these states:

```
Starting ──► Running ──► Ready
   │            │
   ▼            ▼
 Exited       Crashed    (non-zero exit / SIGKILL)
                  Killed  (explicit kill)
```

A job in any of `Starting`, `Running`, or `Ready` is considered "alive." `Exited`, `Crashed`, and `Killed` are terminal states.
