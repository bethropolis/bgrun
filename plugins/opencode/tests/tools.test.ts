import { describe, expect, test } from "bun:test";
import { bgrunTools, resolveJobIdInList } from "../lib/tools.ts";

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
  // Plugin-only: opt an already-running job into completion notifications.
  "watch",
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

describe("resolveJobIdInList", () => {
  const jobs = [
    { id: "abcdef01-1111", name: "web" },
    { id: "abcdef02-2222", name: "worker" },
    { id: "99990000-3333", name: null },
  ];

  test("mirrors daemon resolution: uuid, name, unique prefix", () => {
    expect(resolveJobIdInList(jobs, "abcdef01-1111")).toBe("abcdef01-1111");
    expect(resolveJobIdInList(jobs, "web")).toBe("abcdef01-1111");
    expect(resolveJobIdInList(jobs, "99990000")).toBe("99990000-3333");
    // Ambiguous prefix resolves to nothing (daemon rejects it too).
    expect(resolveJobIdInList(jobs, "abcdef")).toBeUndefined();
    expect(resolveJobIdInList(jobs, "nope")).toBeUndefined();
    expect(resolveJobIdInList(jobs, "abc")).toBeUndefined(); // too short
    expect(resolveJobIdInList({ not: "array" }, "web")).toBeUndefined();
  });
});

describe("bgrunTools registry", () => {
  test("covers all 15 daemon commands plus plugin-only watch", () => {
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
