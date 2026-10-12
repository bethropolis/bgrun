# bgrun OpenCode plugin

Native background-job tools for OpenCode. No MCP hop, no build step — OpenCode
hot-reloads `index.ts` on change.

## Install

Project-local (`opencode.jsonc`):

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "plugins": ["./plugins/opencode"],
}
```

With options (binary resolution: `bgrunPath` → `$BGRUN_BIN` → `PATH`):

```jsonc
{
  "plugins": [{ "package": "./plugins/opencode", "options": { "bgrunPath": "/home/user/.local/bin/bgrun" } }],
}
```

The daemon auto-starts on the first tool call, same as the CLI.

## What it registers

- **16 tools** (`bgrun_*` namespace): `run` (with `wait:true` folding),
  `list` (trimmed by default, `verbose:true` for full records), `status`,
  `kill`, `stop`, `restart`, `tail` (`digest:true` alone returns only the
  summary), `diff`, `wait`, `send`, `stats`, `run_group`, `expect`, `screen`,
  `clean`, plus plugin-only `watch` (opt an already-running job into
  completion notifications). Same 64KB / 1000-line token budget as `bgrun mcp`
  (`crates/bgrun-cli/src/mcp.rs`).
- **Skill** `bgrun` — synced copy of `docs/bgrun/SKILL.md`.
- **Commands** `/bg-run`, `/bg-tail` — guidance wrappers that steer the
  agent toward the right tool.

## Beyond MCP: completion notifications

MCP tools are stateless and session-blind. The plugin records which session
started each job (`run` / `run_group` / `restart` all track; `kill` / `stop`
untrack so your own termination never pings you) and a watcher polls the
daemon, writing the outcome back into that exact session:

```
[bgrun] job "web" (abc123) is ready. Use bgrun_tail / bgrun_diff for logs ...
[bgrun] job "web" (abc123) crashed (exit 1). Use bgrun_tail with level "error" ...
```

The fire-and-continue pattern this enables:

```
bgrun_run(compile, tests)  → returns immediately
... agent does other work ...
[notification arrives]     → agent picks up results, no polling
```

Tracking persists in plugin storage across reloads/restarts; dead sessions
untrack instead of retrying; the loop only hits the daemon while jobs are
tracked. Per call, pass `notify: false` to opt out.

## Interactive sessions (PTY)

`run` with `pty: true` (+ optional `pty_cols`/`pty_rows`), then loop
`expect` → `screen` → `send`. `expect` matches prompts the instant they
print (even mid-line); `screen` takes `strip_ansi: true` for clean reads;
`send` takes raw bytes (`\n` Enter, `\x1b` Esc, `\x1b[A` arrows, `\x03`
Ctrl+C). Full playbook with key table: `docs/bgrun/SKILL.md`
("#Interactive sessions (PTY)").

## Prompt steering

An append-only prompt hook nudges unmistakable foreground long-runners
(`sleep N`, `npm run dev`, `cargo run`, bare `docker run`, trailing `&`,
`tail -f`) toward `bgrun_run`. It never rewrites prompts and skips the
plugin's own `[bgrun]` messages.

## Options

| Option | Default | Meaning |
|---|---|---|
| `bgrunPath` | `$BGRUN_BIN` → `PATH` | Binary override |
| `notify` | `"queue"` | `"queue"` (queued prompt wakes the session), `"synthetic"` (transcript note only), `false` (off by default; per-call `notify: true` still works) |
| `pollMs` | `5000` | Watcher interval (min 1000) |
| `steer` | `true` | Prompt-hook steering (`false` to silence) |

## Develop

```bash
cd plugins/opencode
bun install        # @opencode/plugin, types
bun test           # unit + live daemon round-trip (needs `bgrun` on PATH)
bunx tsc --noEmit  # typecheck
```

Edit `index.ts` / `lib/*.ts` and OpenCode picks it up live. Tool executors
shell out per call, so they need no reload; the skill body watches
`skills/bgrun/SKILL.md` and calls `ctx.skill.reload()` on change.

## Skill sync

The plugin ships a **copy** (npm cannot follow symlinks). The canonical
source is `docs/bgrun/SKILL.md`:

```bash
bun run sync:skill
```

`tests/skill-sync.test.ts` fails when the copy drifts — re-run the sync and
commit both files.
