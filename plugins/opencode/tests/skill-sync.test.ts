import { describe, expect, test } from "bun:test";
import { resolve } from "node:path";

/// Guard against skill drift: the plugin ships a copy of the canonical
/// skill, and npm packaging cannot follow symlinks. If this test fails,
/// run `bun run sync:skill` from `plugins/opencode` and commit both files.
describe("skill sync", () => {
  test("plugin copy matches docs/bgrun/SKILL.md", async () => {
    const canonical = Bun.file(resolve(import.meta.dir, "../../../docs/bgrun/SKILL.md"));
    const copy = Bun.file(resolve(import.meta.dir, "../skills/bgrun/SKILL.md"));
    expect(await copy.exists()).toBe(true);
    expect(await copy.text()).toBe(await canonical.text());
  });
});
