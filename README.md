# bgrun

[![CI](https://github.com/bethropolis/bgrun/actions/workflows/release.yml/badge.svg)](https://github.com/bethropolis/bgrun/actions/workflows/release.yml)
[![Crates.io](https://img.shields.io/crates/v/bgrun-cli.svg)](https://crates.io/crates/bgrun-cli)
[![License: MIT](https://img.shields.io/github/license/bethropolis/bgrun)](LICENSE)

A background process runner for AI agents and automation workflows. Start processes, check status, tail logs, and kill them over a Unix socket with JSON output. The daemon auto-starts on first CLI use.

## Install (Linux, from source)

```bash
./install.sh
```

Requires the Rust toolchain (`cargo`). This builds from a local clone and installs
`bgrun` (CLI) and `bgrun-daemon` (auto-started by the CLI) to `~/.local/bin`.

Optional: install the OpenCode skill bundle from `docs/bgrun/`:

```bash
./install.sh --install-skill
```

## Quick start

```bash
bgrun run --name server --ready-when "listening on" cargo run
# → {"id":"abc123","name":"server","state":"running",...}

bgrun wait abc123 --timeout 30s

bgrun tail abc123 --digest
# → {"total_lines":82,"errors":0,"warnings":2,...}

bgrun kill abc123
```

## Commands

| Command | Purpose |
|---------|---------|
| `bgrun run [flags] <cmd...>` | Start a background process |
| `bgrun run-group <name> <name> ...` | Start multiple named jobs in parallel |
| `bgrun list [--workspace <ws>]` | List all jobs |
| `bgrun status <id>` | Get job state |
| `bgrun wait <id> --timeout <d>` | Block until ready or timeout |
| `bgrun tail <id> [--digest] [--level <l>]` | Show logs |
| `bgrun diff <id>` | Show new log lines since last call |
| `bgrun send <id> <data>` | Write to stdin |
| `bgrun stats <id>` | Show CPU/RSS/uptime |
| `bgrun kill <id> [--workspace <ws>]` | Terminate job(s) |
| `bgrun skill install <dir>` | Install embedded skill bundle |

## Docs

- [Command Reference](docs/commands.md) — flags, examples, output shapes
- [bgrun.toml](docs/bgrun-toml.md) — named job definitions and dependencies
- [Architecture](docs/architecture.md) — daemon, protocol, readiness system

## Crate layout

Workspace crates:

| Crate | Role |
|-------|------|
| `bgrun-proto` | Shared types, no I/O |
| `bgrun-core` | Job state machine, config parser, no I/O |
| `bgrun-daemon` | Spawns/monitors/kills processes, serves the CLI |
| `bgrun-cli` | User-facing CLI |

CLI and daemon communicate over a Unix socket with NDJSON. The daemon stores state in `~/.local/share/bgrun/`.

## License

MIT
