/// Spawn `bgrun` and parse its `--json` output.
///
/// Every invocation passes `--json` explicitly: without it bgrun emits
/// human-readable tables even when piped. `list --json` emits NDJSON (one
/// object per line); every other command emits a single JSON value.

export interface ExecResult {
  ok: boolean;
  stdout: string;
  exitCode: number;
}

export function bgrunBin(options: Record<string, unknown>): string {
  const fromOptions =
    typeof options["bgrunPath"] === "string" && options["bgrunPath"].length > 0
      ? (options["bgrunPath"] as string)
      : undefined;
  const fromEnv = Bun.env["BGRUN_BIN"];
  return fromOptions ?? fromEnv ?? "bgrun";
}

/// Run `bgrun <args...>` with stdin closed. Resolves even on non-zero exit —
/// callers decide how to surface stderr/exit codes as tool errors.
export async function runBgrun(
  bin: string,
  args: string[],
  signal?: AbortSignal,
): Promise<ExecResult> {
  const proc = Bun.spawn([bin, ...args], {
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  if (signal?.aborted) proc.kill();

  const onAbort = () => proc.kill();
  signal?.addEventListener("abort", onAbort, { once: true });

  try {
    const [out, err, exitCode] = await Promise.all([
      new Response(proc.stdout).text(),
      new Response(proc.stderr).text(),
      proc.exited,
    ]);
    return {
      exitCode,
      ok: exitCode === 0,
      stdout: exitCode === 0 ? out : out.length > 0 ? out : err,
    };
  } finally {
    signal?.removeEventListener("abort", onAbort);
  }
}

/// Parse stdout as a single JSON value. Throws a terse error on failure so
/// tool executors can return it directly.
export function parseJson(stdout: string): unknown {
  const trimmed = stdout.trim();
  if (trimmed.length === 0) return null;
  return JSON.parse(trimmed);
}

/// Parse NDJSON (`bgrun list` emits one object per line) into an array.
/// Blank lines are skipped; a single JSON array payload is also accepted.
export function parseNdjson(stdout: string): unknown[] {
  const trimmed = stdout.trim();
  if (trimmed.length === 0) return [];
  if (trimmed.startsWith("[")) {
    const parsed: unknown = JSON.parse(trimmed);
    return Array.isArray(parsed) ? parsed : [parsed];
  }
  return trimmed
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as unknown);
}

/// Extract a daemon-side error message from a failed spawn. Prefers a
/// `{"error": ...}` JSON body, falls back to raw stderr text.
export function errorText(stdout: string): string {
  try {
    const parsed = JSON.parse(stdout.trim()) as { error?: unknown };
    if (typeof parsed?.error === "string" && parsed.error.length > 0) {
      return parsed.error;
    }
  } catch {
    // Not JSON — use the raw text below.
  }
  const trimmed = stdout.trim();
  return trimmed.length > 0 ? trimmed : "bgrun failed with no output";
}
