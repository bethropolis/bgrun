import { describe, expect, test } from "bun:test";
import { bgrunTools } from "../lib/tools.ts";

const EXPECTED_TOOLS = [
  "run",
  "list",
  "status",
  "kill",
  "stop",
  "restart",
  "tail",
  "diff",
  "wait",
  "send",
  "stats",
  "run_group",
  "expect",
  "screen",
  "clean",
];

describe("interactive inputs", () => {
  test("run exposes pty sizing, screen exposes strip_ansi", () => {
    const tools = bgrunTools({ bin: "bgrun" });
    const run = tools.find((t) => t.name === "run")!;
    expect(run.input.properties.pty_cols).toBeDefined();
    expect(run.input.properties.pty_rows).toBeDefined();
    expect(run.input.properties.notify).toBeDefined();
    expect(run.description).toContain("pty");
    const screen = tools.find((t) => t.name === "screen")!;
    expect(screen.input.properties.strip_ansi).toBeDefined();
    const send = tools.find((t) => t.name === "send")!;
    expect(send.description).toContain("u001b");
    const expectTool = tools.find((t) => t.name === "expect")!;
    expect(expectTool.description).toContain(">>>");
  });
});

describe("bgrunTools registry", () => {
  test("covers all 15 daemon commands", () => {
    const tools = bgrunTools({ bin: "bgrun" });
    expect(tools.map((t) => t.name).sort()).toEqual([...EXPECTED_TOOLS].sort());
    for (const tool of tools) {
      expect(tool.description.length).toBeGreaterThan(0);
      expect(tool.input.type).toBe("object");
    }
  });

  test("live list round-trips against the real daemon", async () => {
    const tools = bgrunTools({ bin: "bgrun" });
    const list = tools.find((t) => t.name === "list")!;
    const result = await list.execute(
      {},
      { sessionID: "ses_test", signal: AbortSignal.timeout(15_000) },
    );
    // Trimmed list renders as a compact JSON array.
    expect(result.content.startsWith("[")).toBe(true);
  }, 20_000);
});
