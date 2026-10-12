/// Copy the canonical skill (`docs/bgrun/SKILL.md`) into the plugin.
/// Run with `bun run sync:skill` from `plugins/opencode`.
///
/// The copy is intentional: npm packaging cannot follow repo-relative
/// symlinks, so the plugin ships a snapshot. This script is the sync
/// mechanism — re-run it (or the workspace check below) whenever the
/// canonical skill changes.

import { resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "../..", "..");
const src = Bun.file(resolve(repoRoot, "docs/bgrun/SKILL.md"));
const destPath = resolve(import.meta.dir, "..", "skills/bgrun/SKILL.md");

if (!(await src.exists())) {
  console.error(`canonical skill not found: ${src.name}`);
  process.exit(1);
}

const content = await src.text();
await Bun.write(destPath, content);
console.log(`synced docs/bgrun/SKILL.md -> plugins/opencode/skills/bgrun/SKILL.md (${content.length} bytes)`);
