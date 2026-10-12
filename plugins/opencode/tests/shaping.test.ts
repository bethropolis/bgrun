import { describe, expect, test } from "bun:test";
import {
  MAX_TOOL_LINES,
  MAX_TOOL_TEXT_BYTES,
  applyTextCeiling,
  capLines,
  renderToolText,
  similarJobNames,
  summarizeCmd,
  trimJobList,
} from "../lib/shaping.ts";

describe("capLines", () => {
  test("caps at 1000, floors negatives", () => {
    expect(capLines(10_000)).toBe(1000);
    expect(capLines(5)).toBe(5);
    expect(capLines(-3)).toBe(0);
    expect(capLines(Number.NaN)).toBe(MAX_TOOL_LINES);
  });
});

describe("summarizeCmd", () => {
  test("shortens to three words with ellipsis", () => {
    expect(summarizeCmd(["sh", "-c", "npm run dev --port 3000"])).toBe("sh -c npm …");
    expect(summarizeCmd(["true"])).toBe("true");
    expect(summarizeCmd(null)).toBeNull();
  });
});

describe("trimJobList", () => {
  test("drops env/cwd but keeps full ids", () => {
    const jobs = [
      {
        cmd: ["sh", "-c", "npm run dev --port 3000"],
        cwd: "/home/user/project",
        env: { TERM: "xterm" },
        id: "abcdef01-2345-6789-abcd-ef0123456789",
        name: "web",
        pid: 123,
        started_at: "2026-10-05T00:00:00Z",
        state: "running",
        workspace: null,
      },
    ];
    const trimmed = trimJobList(jobs) as any[];
    expect(trimmed[0].id).toBe("abcdef01-2345-6789-abcd-ef0123456789");
    expect(trimmed[0].cmd).toBe("sh -c npm …");
    expect(trimmed[0]).not.toHaveProperty("env");
    expect(trimmed[0]).not.toHaveProperty("cwd");
    expect(trimmed[0]).not.toHaveProperty("started_at");
  });

  test("passes non-arrays through", () => {
    expect(trimJobList({ ok: true })).toEqual({ ok: true });
  });
});

describe("applyTextCeiling", () => {
  test("marks partial output with a recovery hint", () => {
    const big = "x".repeat(MAX_TOOL_TEXT_BYTES + 100);
    const cut = applyTextCeiling("tail", big);
    expect(cut).toContain("[truncated");
    expect(cut).toContain("fewer `lines`");
    expect(applyTextCeiling("tail", "short")).toBe("short");
  });
});

describe("renderToolText", () => {
  test("renders compact JSON, never pretty-printed", () => {
    const text = renderToolText("status", { a: 1, b: 2 });
    expect(text).toBe('{"a":1,"b":2}');
    expect(renderToolText("status", null)).toBe("ok");
  });
});

describe("similarJobNames", () => {
  test("ranks exact matches first", () => {
    const jobs = [
      { id: "aaaabbbb-1", name: "worker" },
      { id: "web-1111", name: "web" },
      { id: "web-2222", name: "website" },
      { id: "zzzz9999", name: "db" },
    ];
    expect(similarJobNames("web", jobs)).toEqual(["web", "website"]);
    expect(similarJobNames("nomatchatall", jobs)).toEqual([]);
  });
});
