import { describe, expect, test } from "bun:test";
import { bgrunBin, errorText, parseJson, parseNdjson } from "../lib/exec.ts";

describe("bgrunBin", () => {
  test("prefers options over env over PATH", () => {
    expect(bgrunBin({ bgrunPath: "/opt/bgrun" })).toBe("/opt/bgrun");
    const saved = Bun.env["BGRUN_BIN"];
    Bun.env["BGRUN_BIN"] = "/env/bgrun";
    expect(bgrunBin({})).toBe("/env/bgrun");
    if (saved === undefined) delete Bun.env["BGRUN_BIN"];
    else Bun.env["BGRUN_BIN"] = saved;
    expect(bgrunBin({})).toBe("bgrun");
  });
});

describe("parseNdjson", () => {
  test("parses one object per line, skips blanks", () => {
    const out = parseNdjson('{"a":1}\n\n{"a":2}\n');
    expect(out).toEqual([{ a: 1 }, { a: 2 }]);
    expect(parseNdjson("")).toEqual([]);
    expect(parseNdjson('[{"a":1}]')).toEqual([{ a: 1 }]);
  });
});

describe("parseJson / errorText", () => {
  test("round-trips JSON, prefers error field", () => {
    expect(parseJson('{"ok":true}')).toEqual({ ok: true });
    expect(parseJson("")).toBeNull();
    expect(errorText('{"error":"job not found"}')).toBe("job not found");
    expect(errorText("plain stderr")).toBe("plain stderr");
  });
});
