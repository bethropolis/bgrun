/// Token-efficiency shaping, ported from `crates/bgrun-cli/src/mcp.rs`.
/// Keep the constants in sync with `MAX_TOOL_TEXT_BYTES` / `MAX_TOOL_LINES`
/// on the Rust side; the plugin and the MCP server enforce the same budget
/// so agents get identical behavior over either transport.

export const MAX_TOOL_TEXT_BYTES = 65_536;
export const MAX_TOOL_LINES = 1_000;

/// Cap a caller-supplied line count at the per-tool maximum.
export function capLines(lines: number): number {
  if (!Number.isFinite(lines)) return MAX_TOOL_LINES;
  return Math.min(Math.max(0, Math.floor(lines)), MAX_TOOL_LINES);
}

/// Shorten a job's command vector to its first three words with an ellipsis
/// marker when longer. Non-array commands pass through untouched.
export function summarizeCmd(cmd: unknown): unknown {
  if (!Array.isArray(cmd)) return cmd ?? null;
  const words: string[] = [];
  for (const item of cmd) {
    if (typeof item === "string") words.push(...item.split(/\s+/).filter(Boolean));
  }
  if (words.length > 3) return `${words.slice(0, 3).join(" ")} …`;
  return words.join(" ");
}

/// Trim a `list` payload to the fields an agent usually needs. Drops `env`,
/// `cwd`, and timestamps; ids stay whole so they remain usable as handles.
export function trimJobList(jobs: unknown): unknown {
  if (!Array.isArray(jobs)) return jobs;
  return jobs.map((job) => {
    if (typeof job !== "object" || job === null) return job;
    const record = job as Record<string, unknown>;
    return {
      cmd: summarizeCmd(record["cmd"]),
      id: record["id"] ?? null,
      name: record["name"] ?? null,
      pid: record["pid"] ?? null,
      state: record["state"] ?? null,
      workspace: record["workspace"] ?? null,
    };
  });
}

/// Enforce the per-tool output ceiling. Oversized text is cut with a marker
/// naming how to re-query narrower — never silently.
export function applyTextCeiling(toolName: string, text: string): string {
  if (text.length <= MAX_TOOL_TEXT_BYTES) return text;
  // `text` is a JS string (UTF-16); slice at a valid code-point boundary.
  // JSON payloads here are ASCII-dominated, so a char-boundary cut is enough
  // to avoid corrupt output; the marker makes the cut explicit.
  let end = MAX_TOOL_TEXT_BYTES;
  while (end > 0 && isTrailingSurrogate(text.charCodeAt(end))) end -= 1;
  const hint =
    toolName === "tail" || toolName === "diff" || toolName === "screen"
      ? "re-run with fewer `lines` or a filter (`level`, `stream`, `filter_regex`)"
      : toolName === "list"
        ? "re-run with `verbose: false` (default) or a `workspace` filter"
        : "re-run with narrower arguments";
  return `${text.slice(0, end)}\n[truncated: response exceeded ${MAX_TOOL_TEXT_BYTES} bytes; ${hint}]`;
}

function isTrailingSurrogate(code: number): boolean {
  // Low-surrogate range: cutting before one would split a surrogate pair.
  return code >= 0xdc00 && code <= 0xdfff;
}

/// Render a payload as compact JSON (never pretty-printed) under the ceiling.
export function renderToolText(toolName: string, data: unknown): string {
  const text =
    data === null || data === undefined
      ? "ok"
      : typeof data === "string"
        ? data
        : JSON.stringify(data);
  return applyTextCeiling(toolName, text);
}

/// Pick up to three job handles resembling `query` for "did you mean"
/// enrichment. Mirrors `similar_job_names` in `mcp.rs`.
export function similarJobNames(query: string, jobs: unknown): string[] {
  if (!Array.isArray(jobs)) return [];
  const lower = query.toLowerCase();
  const exact: string[] = [];
  const prefix: string[] = [];
  const fuzzy: string[] = [];
  for (const entry of jobs) {
    if (typeof entry !== "object" || entry === null) continue;
    const record = entry as Record<string, unknown>;
    const name = typeof record["name"] === "string" ? (record["name"] as string) : "";
    const id = typeof record["id"] === "string" ? (record["id"] as string) : "";
    const handle = name.length > 0 ? name : `id ${id.slice(0, 8)}`;
    if (name.toLowerCase().includes(lower)) exact.push(handle);
    else if (id.startsWith(query)) prefix.push(handle);
    else if (name.length > 0 && [...lower].every((c) => name.toLowerCase().includes(c))) {
      fuzzy.push(handle);
    }
  }
  return [...exact, ...prefix, ...fuzzy].slice(0, 3);
}
