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

bgrun runs commands as detached background jobs managed by a persistent daemon.
The CLI returns immediately; the daemon tracks the process, captures logs, and
monitors readiness. Output is NDJSON when piped, human-readable on TTY.

## Core workflow

```bash
bgrun run -n server -w "listening on" cargo run     # start + get job ID
bgrun wait <id> --timeout 30s                        # block until ready
bgrun tail <id> --digest                             # check logs cheaply
bgrun kill <id>                                      # clean up
```

## Common commands

| Command | Purpose |
|---------|---------|
| `bgrun run [flags] <cmd...>` | Start a background job |
| `bgrun list [-w <ws>]` | List all jobs |
| `bgrun status <id>` | Get job state (starting/running/ready/exited/crashed/killed) |
| `bgrun wait <id> --timeout <duration>` | Block until ready (not a fixed sleep!) |
| `bgrun tail <id> [--digest] [--level <l>]` | Read logs (use --digest for token savings) |
| `bgrun diff <id>` | New log lines since last call |
| `bgrun send <id> [data] [--enter]` | Write to stdin |
| `bgrun stats <id>` | CPU%/RSS/uptime |
| `bgrun kill <id> [-w <ws>]` | Terminate job(s) |
| `bgrun attach <id>` | Interactive PTY attach |
| `bgrun screen <id> [--lines N]` | Non-blocking in-memory buffer peek |
| `bgrun expect <id> <pattern>` | Wait for log pattern |
| `bgrun clean [--workspace <ws>]` | Remove all terminated jobs |
| `bgrun run-group <name> [name...]` | Start multiple named jobs in parallel |

### `bgrun run` flags

| Flag | Purpose |
|------|---------|
| `-n, --name <name>` | Name the job (idempotent: returns existing if alive) |
| `-w, --workspace <ws>` | Group tag for bulk operations |
| `-r, --ready-when <pattern>` | Wait for this substring in output |
| `--ready-when-regex <regex>` | Wait for regex match in output |
| `--ready-when-port <port>` | Wait for TCP port |
| `--ready-when-url <url>` | Wait for HTTP 2xx |
| `--ready-when-file <path>` | Wait for file to exist |
| `--after <name>` | Don't start until named job is ready |
| `--restart on-crash` | Auto-restart on non-zero exit |
| `--backoff <duration>` | Delay between restarts |
| `--pty` | Allocate pseudo-terminal |
| `--max-rss <mb>` | Kill if RSS exceeds this threshold |
| `--max-runtime <duration>` | Kill after this duration |
| `--allocate-port <name>` | Allocate free port as env var |
| `--health-check-url <url>` | Liveness HTTP probe |
| `--health-check-port <port>` | Liveness TCP probe |
| `--health-interval <secs>` | Seconds between health checks |
| `--health-threshold <n>` | Consecutive failures before kill |

## Sending input to jobs

```bash
bgrun send <id> <text>           # send text (no trailing newline)
bgrun send <id> <text> --enter   # send text + Enter
bgrun send <id> --enter          # just press Enter
bgrun send <id> <text> --newline # send text + newline
```

## Log reading — token efficient

```bash
bgrun tail <id> --digest                   # counts only: total_lines, errors, warnings
bgrun tail <id> --digest --lines 5         # digest + last 5 lines
bgrun tail <id> --level error --lines 5    # filtered lines
bgrun tail <id> --follow                   # live streaming
bgrun diff <id>                            # only new lines since last call
bgrun screen <id> --lines 5               # from in-memory buffer (no disk I/O)
```

## `bgrun.toml` project config

Define named jobs in project root for reusable config:

```toml
[jobs.server]
cmd = "cargo run --release"
ready-when = "listening on"
workspace = "myproject"

[jobs.db]
cmd = "docker run --rm -p 5432:5432 postgres:16"
ready-when-port = 5432
```

Then: `bgrun run server`, `bgrun run-group server db`, `bgrun kill --workspace myproject`

## Agent workflows

**Start server, test, clean up:**
```bash
bgrun run -n srv -r "listening on" cargo run
bgrun wait <id> --timeout 30s   # not sleep!
cargo test --test integration
bgrun kill <id>
```

**Multi-service stack:**
```bash
bgrun run -n db --ready-when-port 5432 docker run postgres:16
bgrun run -n srv --after db -r "listening on" cargo run
bgrun wait <srv-id> --timeout 60s
bgrun kill --workspace myproject
```

**Monitor during test:**
```bash
bgrun run -n srv -r "listening on" ./server
bgrun screen <srv-id>          # non-blocking peek (no I/O)
bgrun diff <srv-id>            # call in loop — only new lines
```

## Anti-patterns

| Don't | Do |
|-------|-----|
| `cargo run &; sleep 10` | `bgrun run -r "pattern" cargo run; bgrun wait <id>` |
| Fixed-sleep polling | `bgrun wait <id>` |
| Re-reading full logs | `bgrun tail --digest` or `bgrun diff` |
| Blocking tail for quick check | `bgrun screen <id>` (non-blocking, no disk I/O) |
| `kill $(pid)` | `bgrun kill <id>` (signals process group) |

## State

| State | Meaning | Alive |
|-------|---------|-------|
| starting | Spawned, not yet running | yes |
| running | Running, no readiness signal yet | yes |
| ready | Readiness signal fired | yes |
| exited | Exit code 0 | no |
| crashed | Non-zero exit | no |
| killed | Terminated by user | no |

Persists across daemon restarts in `~/.local/share/bgrun/`.
