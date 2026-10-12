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

- **15 tools** (`bgrun_*` namespace): `run` (with `wait:true` folding),
  `list` (trimmed by default, `verbose:true` for full records), `status`,
  `kill`, `stop`, `restart`, `tail` (`digest:true` alone returns only the
  summary), `diff`, `wait`, `send`, `stats`, `run_group`, `expect`, `screen`,
  `clean`. Same 64KB / 1000-line token budget as `bgrun mcp`
  (`crates/bgrun-cli/src/mcp.rs`).
- **Skill** `bgrun` — synced copy of `docs/bgrun/SKILL.md`.
- **Commands** `/bg-run`, `/bg-tail` — guidance wrappers that steer the
  agent toward the right tool.

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
