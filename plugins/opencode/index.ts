/// bgrun native tools for OpenCode.
///
/// Loads as a local plugin directory (`plugins/opencode`):
/// ```jsonc
/// { "plugins": ["./plugins/opencode"] }
/// ```
/// No build step — OpenCode hot-reloads this entry on change. Pure TS,
/// `bun`-native (uses `Bun.spawn` / `Bun.file` / `Bun.watch`).
///
/// Binary resolution: plugin option `bgrunPath` → `$BGRUN_BIN` → `PATH`.
/// The daemon auto-starts on the first tool call, same as the CLI.

import { Plugin } from "@opencode/plugin";
import { bgrunBin } from "./lib/exec.ts";
import { bgrunTools } from "./lib/tools.ts";

const SKILL_ID = "bgrun";
const SKILL_NAME = "bgrun";
const SKILL_DESCRIPTION =
  "Use bgrun for long-running processes (servers, builds, test runners, workers). Detaches immediately; poll with diff/tail --digest; wait blocks until ready, never sleep.";

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

export default Plugin.define({
  id: "bgrun",
  async setup(ctx) {
    const bin = bgrunBin(ctx.options);
    const state = { skillContent: await loadSkillContent() };
    const tools = bgrunTools({ bin });

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
        description: "Start a background job with bgrun (detached, returns an id).",
        name: "bg-run",
        async execute({ sessionID, prompt, delivery }) {
          await ctx.session.prompt({
            ...prompt,
            delivery,
            sessionID,
            text: `Use the bgrun_run tool to start this in the background, then bgrun_wait for readiness:\n\n${prompt.text}`,
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
        description: "Detached background jobs: run, poll logs, wait for readiness, stop.",
        name: "bgrun",
      });
      for (const tool of tools) {
        editor.add({
          description: tool.description,
          // The transform callback is synchronous; executors close over the
          // resolved binary and report progress per call.
          execute: async (input, context) => {
            await context.progress({ status: `bgrun ${tool.name}` });
            return tool.execute(input as Record<string, any>, { signal: context.signal });
          },
          input: tool.input as any,
          name: tool.name,
          options: { codemode: true, namespace: "bgrun" },
        });
      }
    });

    // Hot-reload the skill body when the synced copy changes, without an
    // OpenCode restart. Tool executors shell out per call, so they need no
    // reload — only the registered skill content is captured state.
    let watcher: { close(): void } | undefined;
    try {
      const skillPath = new URL("./skills/bgrun/SKILL.md", import.meta.url).pathname;
      const fs = await import("node:fs");
      watcher = fs.watch(skillPath, async () => {
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
      try {
        watcher?.close();
      } catch {
        // Ignore close errors during unload.
      }
    };
  },
});
