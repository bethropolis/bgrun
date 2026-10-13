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

describe("kill/stop untrack-before-terminate", () => {
  const dir = `/tmp/bgrun-stub-${process.pid}`;
  const stub = `${dir}/bgrun`;
  const log = `${dir}/calls.log`;

  async function writeStub(mode: "ok" | "fail-kill"): Promise<void> {
    await Bun.write(
      stub,
      `#!/bin/sh\necho "$@" >> "${log}"\n` +
        `if [ "$1" = "list" ]; then printf '%s\\n' '{"id":"uuid-1","name":"web","state":"running","workspace":null}'; exit 0; fi\n` +
        `if [ "$1" = "kill" ]; then\n` +
        (mode === "ok"
          ? `  echo '{"killed":["web"]}'; exit 0;\n`
          : `  echo 'Error: kill: job not found' >&2; exit 1;\n`) +
        `fi\necho '{}'; exit 0\n`,
    );
    await Bun.$`chmod +x ${stub}`.quiet();
    await Bun.write(log, "");
  }

  function tracker() {
    const events: Array<{ kind: string; id: string; at: number }> = [];
    return {
      events,
      deps: {
        bin: stub,
        tracker: {
          defaultMode: "queue" as const,
          track: async (id: string) => {
            events.push({ at: Date.now(), id, kind: "track" });
          },
          untrack: async (id: string) => {
            events.push({ at: Date.now(), id, kind: "untrack" });
          },
          prune: async () => {},
        },
      },
    };
  }

  test("kill-by-name untracks the UUID before terminating", async () => {
    await writeStub("ok");
    const t = tracker();
    const tools = bgrunTools(t.deps);
    await tools
      .find((tool) => tool.name === "kill")!
      .execute({ id: "web" }, { sessionID: "ses_test", signal: AbortSignal.timeout(5000) });
    const untrack = t.events.find((e) => e.kind === "untrack");
    // Resolved to UUID, not the passed name.
    expect(untrack?.id).toBe("uuid-1");
    // Untrack happened before the kill call hit the stub.
    const calls = (await Bun.file(log).text()).split("\n");
    const killIdx = calls.findIndex((line) => line.startsWith("kill "));
    expect(killIdx).toBeGreaterThanOrEqual(0);
    const killAt = (await Bun.file(log).stat()).mtime.getTime();
    expect(untrack!.at).toBeLessThanOrEqual(killAt);
  });

  test("failed kill restores tracking", async () => {
    await writeStub("fail-kill");
    const t = tracker();
    const tools = bgrunTools(t.deps);
    await expect(
      tools
        .find((tool) => tool.name === "kill")!
        .execute({ id: "web" }, { sessionID: "ses_test", signal: AbortSignal.timeout(5000) }),
    ).rejects.toThrow();
    const kinds = t.events.map((e) => e.kind);
    // untrack first, then re-track when the kill fails.
    expect(kinds).toEqual(["untrack", "track"]);
    expect(t.events[1].id).toBe("uuid-1");
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
