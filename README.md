# bgrun

[![CI](https://github.com/bethropolis/bgrun/actions/workflows/release.yml/badge.svg)](https://github.com/bethropolis/bgrun/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/github/license/bethropolis/bgrun)](LICENSE)

> **Run long-running background tasks easily. Perfect for development servers, test suites, and AI agent automation loops.**

`bgrun` manages your background tasks so you can keep working without terminal lockups. Start servers, track their readiness, tail logs, and shut them down; all over an automatic background daemon using structured JSON payloads.

## Install

```bash
# Quick install (Linux)
curl -fsSL https://bethropolis.github.io/bgrun/install.sh | sh

# AUR (Arch Linux)
yay -S bgrun-bin

# Homebrew (Linux)
brew tap bethropolis/tap && brew install --cask bethropolis/tap/bgrun

# From source
./install.sh   # requires Rust toolchain
```

## Quick start

### 1. Run a Process
Launch a process in the background and tell `bgrun` exactly when it is ready to handle traffic:
```bash
bgrun run --name dev-server --ready-when-port 3000 "npm run dev"
```

### 3. Coordinate Your Workflow
Block execution only until the server is actually ready, inspect its logs, or terminate it cleanly:
```bash
# Block until ready (not a must!)
bgrun wait dev-server --timeout 30s

# Check on-demand logs
bgrun tail dev-server --lines 10

# Clean up
bgrun kill dev-server
```

---

## Commands

| Command | Purpose |
|---------|---------|
| `bgrun run [flags] <cmd...>` | Start a background job |
| `bgrun list [--workspace <ws>]` | List all jobs |
| `bgrun status <id>` | Get job state |
| `bgrun wait <id> --timeout <d>` | Block until ready |
| `bgrun tail <id> [--digest] [--level <l>]` | Show logs |
| `bgrun diff <id>` | New log lines since last call |
| `bgrun send <id> <data>` | Write to stdin |
| `bgrun stats <id>` | Show CPU/RSS/uptime |
| `bgrun kill <id> [--workspace <ws>]` | Terminate job(s) |
| `bgrun attach <id>` | Attach to PTY job interactively |
| `bgrun expect <id> <pattern>` | Wait for log line matching pattern |
| `bgrun schema <command>` | Print JSON Schema for command args |

## Docs

- [Command Reference](docs/commands.md)
- [bgrun.toml](docs/bgrun-toml.md)
- [Architecture](docs/architecture.md)
- [OpenCode Skill](docs/bgrun/SKILL.md)

## Crate layout

| Crate | Role |
|-------|------|
| `bgrun-proto` | Shared types, no I/O |
| `bgrun-core` | Job state machine, config parser |
| `bgrun-daemon` | Spawns/monitors/kills processes, serves CLI |
| `bgrun-cli` | User-facing CLI |

## License

MIT
