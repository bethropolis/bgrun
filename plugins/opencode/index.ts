/// bgrun native tools for OpenCode.
///
/// Loads as a local plugin directory (`plugins/opencode`):
/// ```jsonc
/// { "plugins": ["./plugins/opencode"] }
/// ```
/// No build step — OpenCode hot-reloads this entry on change. Pure TS,
/// `bun`-native (uses `Bun.spawn` / `Bun.file` / `node:fs` watch).
///
/// Beyond the MCP surface, the plugin is session-aware:
/// - every `run`/`run_group`/`restart` records the originating session and
///   the watcher reports ready/exit back into that exact session, so the
///   agent can fire-and-continue instead of blocking in `wait`;
/// - an opt-out prompt hook steers unmistakable long-running shell patterns
///   (`sleep N`, `npm run dev`, trailing `&`) toward the bgrun tools.
///
/// Options (`{ "package": "./plugins/opencode", "options": { ... } }`):
/// - `bgrunPath`: binary override (`$BGRUN_BIN`, then `PATH`, otherwise).
/// - `notify`: `"queue"` (default — queued prompt wakes the session),
///   `"synthetic"` (transcript note only), or `false` (no tracking).
/// - `pollMs`: watcher interval, default 5000.
/// - `steer`: prompt-hook steering, default true.
///
/// Binary resolution: plugin option `bgrunPath` → `$BGRUN_BIN` → `PATH`.
/// The daemon auto-starts on the first tool call, same as the CLI.

import { Plugin } from "@opencode/plugin";
import { bgrunBin, parseNdjson, runBgrun } from "./lib/exec.ts";
import { JobWatcher, type NotifyMode, type TrackedJob } from "./lib/notify.ts";
import { bgrunTools } from "./lib/tools.ts";

const SKILL_ID = "bgrun";
const SKILL_NAME = "bgrun";
const SKILL_DESCRIPTION =
  "Use bgrun for long-running processes (servers, builds, test runners, workers). Detaches immediately and notifies this session on ready/exit; poll with diff/tail --digest; wait blocks until ready, never sleep.";
const TRACKED_KEY = "bgrun.tracked.v1";

type NotifyOption = NotifyMode | false;

function notifyOption(options: Record<string, unknown>): NotifyOption {
  const raw = options["notify"];
  if (raw === false || raw === "synthetic" || raw === "queue") return raw;
  return "queue";
}

function pollMsOption(options: Record<string, unknown>): number {
  const raw = options["pollMs"];
  if (typeof raw === "number" && Number.isFinite(raw) && raw >= 1000) return Math.floor(raw);
  return 5000;
}

async function loadSkillContent(): Promise<string> {
  const url = new URL("./skills/bgrun/SKILL.md", import.meta.url);
  try {
    const file = Bun.file(url);
    if (await file.exists()) return await file.text();
  } catch {
    // Fall through to the embedded fallback below.
  }
  return `# bgrun\n\n${SKILL_DESCRIPTION}\n`;
}

/// Unmistakable "this will hang the turn" shell patterns. Conservative on
/// purpose: only matches when backgrounding is clearly right.
function steerHint(text: string): string | null {
  const lines = text.split("\n").map((line) => line.trim());
  const hits: string[] = [];
  for (const line of lines) {
    if (/(^|;|&&|\|\|)\s*sleep\s+\d+/.test(line)) hits.push("sleep");
    else if (/\b(npm|pnpm|yarn|bun)\s+run\s+dev\b/.test(line)) hits.push("dev server");
    else if (/\bcargo\s+(run|watch)\b/.test(line)) hits.push("cargo run");
    else if (/(^|\s)docker\s+run\b/.test(line) && !/\s-d\b/.test(line)) hits.push("docker run");
    else if (/&\s*$/.test(line) && line.length > 1) hits.push("trailing &");
    else if (/\btail\s+-f\b/.test(line)) hits.push("tail -f");
  }
  if (hits.length === 0) return null;
  const unique = [...new Set(hits)];
  return (
    `\n\n[bgrun] This looks like a long-running command (${unique.join(", ")}). ` +
    `Prefer the bgrun_run tool (detached, auto-notifies this session on ready/exit) ` +
    `over blocking the turn. (Set the bgrun plugin option "steer": false to silence this.)`
  );
}

export default Plugin.define({
  id: "bgrun",
  async setup(ctx) {
    const bin = bgrunBin(ctx.options);
    const notifyDefault = notifyOption(ctx.options);
    const pollMs = pollMsOption(ctx.options);
    const steer = ctx.options["steer"] !== false;
    const state = { skillContent: await loadSkillContent() };

    const watcher = new JobWatcher({
      bin,
      pollMs,
      listJobs: async () => {
        const result = await runBgrun(bin, ["list", "--json"]);
        if (!result.ok) throw new Error(result.stdout.trim() || "bgrun list failed");
        const map = new Map<string, Record<string, any>>();
        for (const job of parseNdjson(result.stdout)) {
          const record = job as Record<string, any>;
          if (typeof record["id"] === "string") map.set(record["id"] as string, record);
        }
        return map;
      },
      load: async () => {
        const saved = (await ctx.storage.get(TRACKED_KEY)) as unknown;
        if (saved && typeof saved === "object" && !Array.isArray(saved)) {
          return saved as Record<string, TrackedJob>;
        }
        return {};
      },
      save: async (tracked) => {
        // TrackedJob is plain JSON at runtime; the double cast bridges the
        // effect-Schema Json type without threading codecs through here.
        await ctx.storage.set(TRACKED_KEY, tracked as unknown as any);
      },
      notify: async (sessionID, text, mode) => {
        if (mode === "synthetic") {
          await ctx.session.synthetic({ sessionID, text } as any);
        } else {
          await ctx.session.prompt({ delivery: "queue", sessionID, text } as any);
        }
      },
    });
    await watcher.restore();
    watcher.start();

    const tools = bgrunTools({
      bin,
      tracker: {
        defaultMode: notifyDefault,
        prune: () => watcher.prune(),
        track: (id, entry) => watcher.track(id, entry),
        untrack: (id) => watcher.untrack(id),
      },
    });

    await ctx.skill.transform((editor) => {
      // Branded IDs/paths are runtime-validated by OpenCode; the cast keeps
      // this call site readable instead of threading effect Brand makers.
      editor.add({
        content: state.skillContent,
        description: SKILL_DESCRIPTION,
        id: SKILL_ID,
        name: SKILL_NAME,
        path: new URL("./skills/bgrun/SKILL.md", import.meta.url).pathname,
      } as any);
    });

    await ctx.command.transform((editor) => {
      editor.add({
        description: "Start a background job with bgrun (detached, notifies on ready/exit).",
        name: "bg-run",
        async execute({ sessionID, prompt, delivery }) {
          await ctx.session.prompt({
            ...prompt,
            delivery,
            sessionID,
            text: `Use the bgrun_run tool to start this in the background (it notifies this session on ready/exit), then continue with other work instead of waiting:\n\n${prompt.text}`,
          } as any);
        },
      });
      editor.add({
        description: "Check a background job's log cheaply (digest first, then lines).",
        name: "bg-tail",
        async execute({ sessionID, prompt, delivery }) {
          await ctx.session.prompt({
            ...prompt,
            delivery,
            sessionID,
            text: `Use bgrun_tail with digest:true first for this job, then fetch lines only if needed:\n\n${prompt.text}`,
          } as any);
        },
      });
    });

    await ctx.tool.transform((editor) => {
      editor.namespace({
        description:
          "Detached background jobs: run, poll logs, wait for readiness, stop. Runs auto-notify this session on ready/exit.",
        name: "bgrun",
      });
      for (const tool of tools) {
        editor.add({
          description: tool.description,
          // The transform callback is synchronous; executors close over the
          // resolved binary and watcher, and report progress per call.
          execute: async (input, context) => {
            await context.progress({ status: `bgrun ${tool.name}` });
            return tool.execute(input as Record<string, any>, {
              sessionID: context.sessionID as string,
              signal: context.signal,
            });
          },
          input: tool.input as any,
          name: tool.name,
          options: { codemode: true, namespace: "bgrun" },
        });
      }
    });

    // Steer unmistakable foreground long-runners toward bgrun. Append-only:
    // never rewrites the user's prompt. Skips our own notifier messages.
    if (steer) {
      await ctx.session.hook("prompt", (event) => {
        const text = event.prompt.text ?? "";
        if (text.includes("[bgrun]")) return;
        const hint = steerHint(text);
        if (hint) event.prompt.text = `${text}${hint}`;
      });
    }

    // Hot-reload the skill body when the synced copy changes, without an
    // OpenCode restart. Tool executors shell out per call, so they need no
    // reload — only the registered skill content is captured state.
    let watcherHandle: { close(): void } | undefined;
    try {
      const skillPath = new URL("./skills/bgrun/SKILL.md", import.meta.url).pathname;
      const fs = await import("node:fs");
      watcherHandle = fs.watch(skillPath, async () => {
        try {
          state.skillContent = await loadSkillContent();
          await ctx.skill.reload();
        } catch (error) {
          console.error(`[bgrun] skill reload failed: ${error}`);
        }
      });
    } catch {
      // Watching is best-effort (e.g. read-only installs); setup still succeeds.
    }

    return () => {
      watcher.stop();
      try {
        watcherHandle?.close();
      } catch {
        // Ignore close errors during unload.
      }
    };
  },
});
