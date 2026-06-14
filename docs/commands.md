# Command Reference

bgrun has 18 subcommands. All commands return JSON to stdout when piped, or human-readable output when connected to a terminal.

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
| `--ready-when-regex <REGEX>` | Mark job `Ready` when a log line matches this regex |
| `--ready-when-port <PORT>` | Mark job `Ready` when TCP port `localhost:PORT` accepts connections |
| `--ready-when-url <URL>` | Mark job `Ready` when GET returns HTTP 2xx |
| `--ready-when-file <PATH>` | Mark job `Ready` when file exists |
| `--after <NAME>` | Wait for named job to reach `Ready` (or `Exited`/`Crashed`) before spawning |
| `--pty` | Allocate a pseudo-terminal (useful for processes that buffer output differently with pipes). The PTY is allocated by bgrun's `portable-pty` library. **Known limitation:** programs that open their own PTY (e.g. `podman exec -it`, `ssh`, `docker attach`) may not work with `--pty` because the child's PTY is consumed by the inner command rather than bgrun's PTY master. |
| `--restart on-crash` | Auto-restart if the process exits non-zero (SIGKILL, crash, non-zero exit) |
| `--backoff <DURATION>` | Delay between restart attempts, e.g. `2s`, `5m`, `500ms` (default: `2s`, only with `--restart`) |
| `--max-rss <MB>` | Kill the job if resident memory exceeds this threshold (checked every 1s) |
| `--max-runtime <D>` | Kill the job after this duration (e.g. `30s`, `5m`) |
| `--cols <N>` | PTY width in columns (default: 80, only with `--pty`) |
| `--rows <N>` | PTY height in rows (default: 24, only with `--pty`) |
| `--allocate-port <NAME>` | Allocate a free TCP port and set it as the environment variable `<NAME>` |
| `--health-check-url <URL>` | Poll an HTTP URL periodically for liveness after ready |
| `--health-check-port <PORT>` | Probe a TCP port periodically for liveness after ready |
| `--health-interval <SECS>` | Seconds between health checks (default: 10) |
| `--health-threshold <N>` | Consecutive health check failures before killing (default: 3) |

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
bgrun status [<ID>] [--name <NAME>]
```

**Flags**

| Flag | Description |
|---|---|
| `--name / -n <NAME>` | Job name (alternative to positional ID) |

**Examples**

```bash
bgrun status abc123
bgrun status server  # works with names too
bgrun status --name server
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
bgrun tail <ID> [--lines <N>] [--digest] [--level <LEVEL>] [--stream <S>] [--strip-ansi] [--follow] [--filter-regex <R>]
```

**Flags**

| Flag | Description |
|---|---|
| `--lines <N>` | Number of lines to show (default: 20) |
| `--digest` | Show summary instead of raw lines (error/warn count, last error) |
| `--level <LEVEL>` | Filter lines containing `error` or `warn` (case-insensitive) |
| `--stream <S>` | Filter by stream source (`stdout`, `stderr`, `pty`) |
| `--strip-ansi` | Strip ANSI escape codes from output |
| `--follow` | Follow new log lines in real time |
| `--filter-regex <R>` | Filter log lines matching a regex pattern |

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
bgrun diff <ID> [--lines <N>] [--stream <S>] [--strip-ansi] [--filter-regex <R>]
```

**Flags**

| Flag | Description |
|---|---|
| `--lines <N>` | Number of lines to show (unlimited if not set) |
| `--stream <S>` | Filter by stream source (`stdout`, `stderr`, `pty`) |
| `--strip-ansi` | Strip ANSI escape codes from output |
| `--filter-regex <R>` | Filter log lines matching a regex pattern |

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

Block until a job reaches `Ready` state, or a timeout elapses. If the job exits or crashes before becoming ready, returns immediately with the terminal state and exit code.

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
{"ready":true,"elapsed_ms":1234,"exit_code":null,"state":null}
```

If the job exits with code 0 before becoming Ready (pattern not matched):

```json
{"ready":false,"elapsed_ms":350,"exit_code":0,"state":"exited"}
```

If the job crashes before becoming Ready:

```json
{"ready":false,"elapsed_ms":150,"exit_code":1,"state":"crashed"}
```

If the timeout elapses while the job is still Running:

```json
{"ready":false,"elapsed_ms":60000,"exit_code":null,"state":null}
```

---

## kill

Terminate a job by ID, name, or entire workspace.

```
bgrun kill [<ID>] [--name <NAME>] [--workspace <WS>]
```

**Flags**

| Flag | Description |
|---|---|
| `--name / -n <NAME>` | Job name (alternative to positional ID) |
| `--workspace <WS>` | Kill all jobs in a workspace |

Sends `SIGTERM` first, then `SIGKILL` after 5 seconds if the process hasn't exited. Sends to the entire process group, so child processes are cleaned up.

**Examples**

```bash
# Kill by ID
bgrun kill abc123

# Kill by name
bgrun kill server
bgrun kill --name server

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
bgrun send <ID> [<DATA>] [--newline] [--enter]
```

**Flags**

| Flag | Description |
|---|---|
| `--newline` | Append a newline to the data |
| `--enter` | Send just an Enter (newline), optionally with data |

**Examples**

```bash
# Send text
bgrun send server "/reload"

# Append newline
bgrun send server "/reload" --newline

# Send just Enter (press Return)
bgrun send server --enter

# Send text followed by Enter
bgrun send server "yes" --enter

# Using shell escape
bgrun send server $'yes\n'
```

**Output** (JSON)

```json
{"ok":true}
```

Works with both piped and `--pty` jobs.

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

## skill

Install the embedded skill bundle to a target directory.

```
bgrun skill install <DIR>
```

**Examples**

```bash
bgrun skill install ~/.config/opencode/skills/bgrun
```

**Output**

```
Installed skill to /home/user/.config/opencode/skills/bgrun/SKILL.md
```

---

## attach

Attach to a PTY job's interactive terminal. Enables raw bidirectional communication with a process running in a pseudo-terminal.

```
bgrun attach <ID>
```

**Examples**

```bash
bgrun attach server
```

Once attached, the terminal enters raw mode:
- Keystrokes are forwarded to the PTY job's stdin.
- The job's PTY output is displayed live.
- **Ctrl+C** detaches without killing the job (unlike a normal terminal).
- **Ctrl+\** (SIGQUIT) is also forwarded but may terminate the job.
- Terminal resize events are forwarded to the PTY master.

The connection is closed automatically when the job exits.

---

## screen

Show last N lines from a job's in-memory ring buffer (non-blocking). Unlike `bgrun tail`, this reads from a live memory buffer rather than disk, making it suitable for rapid polling without I/O overhead.

```
bgrun screen <ID> [--lines <N>]
```

**Flags**

| Flag | Description |
|---|---|
| `--lines <N>` | Number of lines to show (default: 20) |

**Examples**

```bash
# Last 20 lines
bgrun screen server

# Last 5 lines
bgrun screen server --lines 5
```

**Output** (JSON)

```json
["line 1", "line 2", "line 3"]
```

Human output prints each line directly.

---

## clean

Remove all terminated (crashed/exited/killed) jobs.

```
bgrun clean [--workspace <WS>] [--force]
```

**Flags**

| Flag | Description |
|---|---|
| `--workspace <WS>` | Only clean jobs in this workspace |
| `-f / --force` | Skip confirmation prompt |

**Examples**

```bash
# Remove all terminated jobs
bgrun clean

# Remove only terminated jobs in a workspace
bgrun clean --workspace myapp

# Skip confirmation
bgrun clean --force
```

**Output** (JSON)

```json
{"removed":["abc123","def456"]}
```

---

## expect

Wait for a pattern to appear in a job's log output. Returns when the pattern is found or the timeout expires.

```
bgrun expect <ID> <PATTERN> [--regex] [--timeout <DURATION>]
```

**Flags**

| Flag | Description |
|---|---|
| `--regex` | Treat pattern as a regular expression |
| `--timeout <D>` | Max wait time, e.g. `30s`, `5m` (default: `60s`) |

**Examples**

```bash
# Wait for substring
bgrun expect server "listening on"

# Wait for regex
bgrun expect server "http://localhost:\d+" --regex

# With custom timeout
bgrun expect server "ready" --timeout 10s
```

**Output** (JSON) — on match:

```json
{"matched":true,"line_number":42,"content":"listening on :8080"}
```

On timeout:

```json
{"matched":false,"line_number":null,"content":null}
```

---

## schema

Print JSON Schema (draft-07) for a command's argument struct. Designed for AI agents to discover the expected input shape at runtime.

```
bgrun schema <COMMAND>
```

Supported commands: `run`, `kill`, `tail`.

**Examples**

```bash
bgrun schema run
bgrun schema kill
bgrun schema tail
```

**Output** (JSON) — a standard JSON Schema document with `title`, `type`, `properties`, and `required` fields.

---

## completions (hidden)

Hidden subcommand for shell autocomplete scripts. Prints tab-separated job information for shell tab-completion, or generates full completion scripts.

```
bgrun completions --active-ids
bgrun completions --workspaces
bgrun completions --shell fish
bgrun completions --shell bash
bgrun completions --shell zsh
```

**Flags**

| Flag | Description |
|------|-------------|
| `--active-ids` | Print active short IDs with state descriptions |
| `--workspaces` | Print unique active workspaces |
| `--shell <fish\|bash\|zsh>` | Generate a complete completion script for the given shell |

**Installation**

```fish
# Fish
bgrun completions --shell fish > ~/.config/fish/completions/bgrun.fish

# Bash
bgrun completions --shell bash | sudo tee /etc/bash_completion.d/bgrun

# Zsh
bgrun completions --shell zsh > /usr/local/share/zsh/site-functions/_bgrun
```

**Dynamic completion integration**

The `--active-ids` and `--workspaces` flags produce live data from the daemon, used by shell functions:

```fish
# In ~/.config/fish/completions/bgrun.fish:
complete -c bgrun -n "__fish_seen_subcommand_from status kill wait tail diff send stats attach expect" -a "(bgrun completions --active-ids)"
complete -c bgrun -n "__fish_seen_subcommand_from list kill; and __fish_prev_arg_in --workspace" -a "(bgrun completions --workspaces)"
```

---

## Interactive menu

Running `bgrun` without any subcommand opens an interactive TUI menu:

- **List & Refresh Jobs** — runs `bgrun list`
- **View Job Status/Stats** — select a job, shows status and resource stats
- **Attach to Interactive PTY** — select a job and attach
- **Tail Job Logs** — select a job, shows last 20 lines
- **Kill a Job** — select a job and kill it
- **Exit Menu**

The job list is populated live from the daemon, showing short ID, name, state, and command.

---

## ID resolution

All commands that accept a job ID also accept:
- **Full UUID** — the canonical job identifier
- **Job name** — as set with `--name`
- **Unique prefix** — at least 4 characters of the UUID that match exactly one job

```bash
bgrun status abc1          # prefix match
bgrun status my-server     # name match
bgrun status abc12345...   # full UUID
```

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
