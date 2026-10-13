/// Native bgrun tool definitions. Registered from `index.ts` inside a single
/// synchronous `ctx.tool.transform` callback; every executor shells to the
/// `bgrun` CLI (piped JSON) and shapes output with `lib/shaping.ts` so the
/// plugin matches the MCP server's token budget over either transport.

import { errorText, parseJson, parseNdjson, runBgrun } from "./exec.ts";
import type { NotifyMode, TrackedJob } from "./notify.ts";
import {
  capLines,
  renderToolText,
  similarJobNames,
  trimJobList,
} from "./shaping.ts";

export interface ToolDeps {
  bin: string;
  /** Watcher hooks. Absent when notifications are fully disabled. */
  tracker?: {
    /** Default delivery when the caller passes no per-call `notify`. */
    defaultMode: NotifyMode | false;
    track: (id: string, entry: TrackedJob) => Promise<void>;
    untrack: (id: string) => Promise<void>;
    prune: () => Promise<void>;
  };
}

export interface ToolExecContext {
  signal: AbortSignal;
  /** Originating OpenCode session — used for completion tracking. */
  sessionID: string;
}

type AnyRecord = Record<string, any>;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

function str(v: unknown): string | undefined {
  return typeof v === "string" && v.length > 0 ? v : undefined;
}

function num(v: unknown): number | undefined {
  return typeof v === "number" && Number.isFinite(v) ? v : undefined;
}

function bool(v: unknown, fallback = false): boolean {
  return typeof v === "boolean" ? v : fallback;
}

/// Run `bgrun <args>`; throw a terse daemon error when the CLI fails.
async function mustRun(
  bin: string,
  args: string[],
  signal?: AbortSignal,
): Promise<string> {
  const result = await runBgrun(bin, args, signal);
  if (!result.ok) throw new Error(errorText(result.stdout));
  return result.stdout;
}

/// Enrich "job not found" failures with near-matching handles so the caller
/// can self-correct without a separate list round trip. Never masks the
/// original error: a failed lookup returns the message untouched.
async function withSuggestions(
  bin: string,
  message: string,
  query: string | undefined,
): Promise<string> {
  if (!query || !message.includes("job not found")) return message;
  try {
    const out = await runBgrun(bin, ["list"]);
    if (!out.ok) return message;
    const suggestions = similarJobNames(query, parseNdjson(out.stdout));
    return suggestions.length > 0 ? `${message}; did you mean: ${suggestions.join(", ")}` : message;
  } catch {
    return message;
  }
}

/// Untrack a job BEFORE a terminating operation, returning a restore hook.
/// A kill-then-untrack order leaves a window where the poller observes the
/// terminal state first and queues a notification for your own kill; the
/// restore hook re-tracks when the operation fails so a still-alive job
/// keeps its completion notification.
async function preUntrack(
  deps: ToolDeps,
  input: AnyRecord,
  bin: string,
  sessionID: string,
  handle: string,
): Promise<{ uuids: string[]; restore: () => Promise<void> }> {
  const tracker = deps.tracker;
  const resolved = await resolveJobIds(bin, handle);
  const uuids = resolved.length > 0 ? resolved : [handle];
  for (const uuid of uuids) {
    await untrackLogged(tracker, uuid, "pre-terminate");
  }
  const mode = notifyModeFor(deps, input);
  return {
    uuids,
    restore: async () => {
      if (!mode || !tracker) return;
      for (const uuid of uuids) {
        await tracker
          .track(uuid, {
            createdAt: Date.now(),
            label: handle,
            mode,
            readyNotified: false,
            sessionID,
          })
          .catch((error) => console.error(`[bgrun] re-track ${uuid} failed: ${error}`));
      }
    },
  };
}

/// UUIDs of jobs currently in a workspace (single list call), for
/// untracking ahead of a bulk `kill --workspace`.
async function workspaceJobIds(bin: string, workspace: string): Promise<string[]> {
  try {
    const result = await runBgrun(bin, ["list", "--json"]);
    if (!result.ok) return [];
    const jobs = parseNdjson(result.stdout);
    if (!Array.isArray(jobs)) return [];
    return jobs
      .filter(
        (job): job is Record<string, unknown> =>
          typeof job === "object" &&
          job !== null &&
          (job as Record<string, unknown>)["workspace"] === workspace &&
          typeof (job as Record<string, unknown>)["id"] === "string",
      )
      .map((job) => job["id"] as string);
  } catch {
    return [];
  }
}

async function failWithSuggestions(bin: string, stdout: string, query?: string): Promise<never> {
  throw new Error(await withSuggestions(bin, errorText(stdout), query));
}

/// Mirror the daemon's id resolution (exact UUID, exact name, unique id
/// prefix of 4+ chars) over a `list --json` payload. Tools accept any handle
/// but tracking is keyed by UUID, so kill/stop/restart must resolve before
/// untracking — otherwise the entry leaks and a dead job notifies later.
export function resolveJobIdInList(jobs: unknown, handle: string): string | undefined {
  return resolveJobIdsInList(jobs, handle)[0];
}

/// All UUIDs a handle can mean. Exact UUID and unique prefix resolve singly;
/// a NAME can match several records at once (stale dead jobs keep their
/// names until cleaned), so every same-named UUID is returned — untracking
/// is idempotent and the daemon kills newest-first, meaning any of them may
/// be the victim.
export function resolveJobIdsInList(jobs: unknown, handle: string): string[] {
  if (!Array.isArray(jobs) || handle.length === 0) return [];
  const named: string[] = [];
  let prefix: string | undefined;
  let prefixAmbiguous = false;
  for (const entry of jobs) {
    if (typeof entry !== "object" || entry === null) continue;
    const record = entry as Record<string, unknown>;
    const id = typeof record["id"] === "string" ? (record["id"] as string) : "";
    const name = typeof record["name"] === "string" ? (record["name"] as string) : "";
    if (id === handle) return [id];
    if (name === handle && id !== "") named.push(id);
    if (handle.length >= 4 && id.startsWith(handle)) {
      if (prefix !== undefined) prefixAmbiguous = true;
      prefix = id;
    }
  }
  if (named.length > 0) return named;
  if (prefix !== undefined && !prefixAmbiguous) return [prefix];
  return [];
}

async function resolveJobId(bin: string, handle: string): Promise<string | undefined> {
  return (await resolveJobIds(bin, handle))[0];
}

async function resolveJobIds(bin: string, handle: string): Promise<string[]> {
  try {
    const result = await runBgrun(bin, ["list", "--json"]);
    if (!result.ok) return [];
    return resolveJobIdsInList(parseNdjson(result.stdout), handle);
  } catch {
    return [];
  }
}

/// Untrack loudly-failing: a swallowed untrack error is how dead jobs Leak
/// into later spurious notifications (the entry survives every reload via
/// persisted storage).
async function untrackLogged(
  tracker: ToolDeps["tracker"],
  id: string,
  why: string,
): Promise<void> {
  try {
    await tracker?.untrack(id);
  } catch (error) {
    console.error(`[bgrun] untrack ${id} after ${why} failed: ${error}`);
  }
}

function idSchema(description = "Job ID, name handle, or unique id prefix (min 4 chars).") {
  return {
    additionalProperties: false,
    properties: { id: { description, type: "string" } },
    required: ["id"],
    type: "object",
  };
}

function timeoutArgs(flag: string, timeoutMs: number | undefined, fallback: number): string[] {
  // CLI takes durations (`30s`); tool inputs are ms. Convert explicitly.
  const ms = num(timeoutMs) ?? fallback;
  return [flag, `${Math.max(0, Math.round(ms))}ms`];
}

// ---------------------------------------------------------------------------
// tool definitions
// ---------------------------------------------------------------------------

export interface BgrunToolDef {
  description: string;
  execute: (input: AnyRecord, context: ToolExecContext) => Promise<{ content: string }>;
  input: AnyRecord;
  name: string;
}

/// Resolve whether a spawn should be completion-tracked: per-call `notify`
/// wins, otherwise the plugin default. Returns the delivery mode or null.
function notifyModeFor(deps: ToolDeps, input: AnyRecord): NotifyMode | null {
  if (typeof input["notify"] === "boolean") return input["notify"] ? "queue" : null;
  const def = deps.tracker?.defaultMode;
  return def === false || def === undefined ? null : def;
}

async function trackSpawn(
  deps: ToolDeps,
  input: AnyRecord,
  sessionID: string,
  record: AnyRecord,
): Promise<void> {
  const mode = notifyModeFor(deps, input);
  const id = typeof record?.["id"] === "string" ? (record["id"] as string) : undefined;
  if (!mode || !id || !sessionID) return;
  const name = typeof record?.["name"] === "string" ? (record["name"] as string) : "";
  await deps.tracker!.track(id, {
    createdAt: Date.now(),
    label: name.length > 0 ? `"${name}" (${id.slice(0, 8)})` : id.slice(0, 8),
    mode,
    readyNotified: false,
    sessionID,
  }).catch((error) => console.error(`[bgrun] track failed: ${error}`));
}

export function bgrunTools(deps: ToolDeps): BgrunToolDef[] {
  const { bin } = deps;

  return [
    {
      description:
        "Start a background job and return its record. `cmd` is the command vector. Set `wait: true` to block until ready in the same call instead of calling bgrun_wait separately. For interactive programs set `pty: true`, then drive them with bgrun_send / bgrun_screen / bgrun_expect. The job is completion-tracked: this session is notified when it becomes ready or exits (pass `notify: false` to opt out).",
      async execute(input, context) {
        const cmd = Array.isArray(input["cmd"])
          ? (input["cmd"] as unknown[]).map(String)
          : undefined;
        if (!cmd || cmd.length === 0) throw new Error("missing required argument: cmd");
        const args = ["run", "--json"];
        const name = str(input["name"]);
        if (name) args.push("--name", name);
        const workspace = str(input["workspace"]);
        if (workspace) args.push("--workspace", workspace);
        const readyWhen = str(input["ready_when"]);
        if (readyWhen) args.push("--ready-when", readyWhen);
        const cwd = str(input["cwd"]);
        if (cwd) args.push("--cwd", cwd);
        if (bool(input["pty"])) args.push("--pty");
        const cols = num(input["pty_cols"]);
        if (cols !== undefined) args.push("--cols", String(Math.max(1, Math.round(cols))));
        const rows = num(input["pty_rows"]);
        if (rows !== undefined) args.push("--rows", String(Math.max(1, Math.round(rows))));
        if (bool(input["sandbox"])) args.push("--sandbox");
        const env = input["env"];
        if (env && typeof env === "object") {
          for (const [k, v] of Object.entries(env as AnyRecord)) args.push("--env", `${k}=${v}`);
        }
        if (bool(input["replace"])) args.push("--replace");
        args.push("--", ...cmd);
        const result = await runBgrun(bin, args, context.signal);
        if (!result.ok) await failWithSuggestions(bin, result.stdout);
        const record = parseJson(result.stdout) as AnyRecord;
        await trackSpawn(deps, input, context.sessionID, record);
        if (!bool(input["wait"])) return { content: renderToolText("run", record) };
        const id = (record as AnyRecord)?.["id"];
        if (typeof id !== "string") throw new Error("started job, but the daemon returned no job id to wait on");
        const waitTimeout = num(input["wait_timeout_ms"]) ?? 60_000;
        const waited = await runBgrun(
          bin,
          ["wait", "--json", id, "--timeout", `${Math.max(0, Math.round(waitTimeout))}ms`],
          context.signal,
        );
        if (!waited.ok) {
          throw new Error(`started job ${id}, but waiting failed: ${errorText(waited.stdout)}`);
        }
        return { content: renderToolText("run", { run: record, wait: parseJson(waited.stdout) }) };
      },
      input: {
        additionalProperties: false,
        properties: {
          cmd: { description: "Command vector, e.g. [\"cargo\", \"run\"].", items: { type: "string" }, type: "array" },
          cwd: { description: "Working directory for the job.", type: "string" },
          env: { additionalProperties: { type: "string" }, description: "Environment variables.", type: "object" },
          name: { description: "Stable handle (idempotent re-run).", type: "string" },
          notify: { description: "Notify this session on ready/exit (default: plugin setting).", type: "boolean" },
          pty: { description: "Allocate a pseudo-terminal (for interactive programs; drive with bgrun_send/bgrun_screen/bgrun_expect).", type: "boolean" },
          pty_cols: { description: "PTY width in columns (default 80, only with pty).", type: "number" },
          pty_rows: { description: "PTY height in rows (default 24, only with pty).", type: "number" },
          ready_when: { description: "Mark ready when a log line contains this substring.", type: "string" },
          replace: { description: "Kill an existing same-name job first.", type: "boolean" },
          sandbox: { description: "Linux network+IPC isolation (piped mode only).", type: "boolean" },
          wait: { description: "Block until ready before returning.", type: "boolean" },
          wait_timeout_ms: { description: "Max wait after starting (default 60000).", type: "number" },
          workspace: { description: "Group tag for bulk operations.", type: "string" },
        },
        required: ["cmd"],
        type: "object",
      },
      name: "run",
    },
    {
      description:
        "List jobs (id, name, state, pid, command). Pass `verbose: true` for the full record including env and cwd.",
      async execute(input, context) {
        const args = ["list", "--json"];
        const workspace = str(input["workspace"]);
        if (workspace) args.push("--workspace", workspace);
        const result = await runBgrun(bin, args, context.signal);
        if (!result.ok) throw new Error(errorText(result.stdout));
        const jobs = parseNdjson(result.stdout);
        const data = bool(input["verbose"]) ? jobs : trimJobList(jobs);
        return { content: renderToolText("list", data) };
      },
      input: {
        additionalProperties: false,
        properties: {
          verbose: { description: "Return full job records (default false).", type: "boolean" },
          workspace: { description: "Only list jobs in this workspace.", type: "string" },
        },
        type: "object",
      },
      name: "list",
    },
    {
      description: "Get a job's state (starting/running/ready/exited/crashed/killed), exit code, and restart counters.",
      async execute(input, context) {
        const id = str(input["id"]);
        if (!id) throw new Error("missing required argument: id");
        const result = await runBgrun(bin, ["status", "--json", id], context.signal);
        if (!result.ok) await failWithSuggestions(bin, result.stdout, id);
        return { content: renderToolText("status", parseJson(result.stdout)) };
      },
      input: idSchema(),
      name: "status",
    },
    {
      description:
        "Terminate a job: SIGTERM first, escalating to SIGKILL after 5s. Fails on already-finished jobs (use bgrun_stop to mark those). Or kill every job in a workspace with `workspace` instead of `id`.",
      async execute(input, context) {
        const workspace = str(input["workspace"]);
        const id = str(input["id"]);
        if (!id && !workspace) throw new Error("missing required argument: id (or workspace)");
        // Untrack ahead of terminating (see preUntrack): bulk kills resolve
        // workspace members up front instead.
        let pre: { uuids: string[]; restore: () => Promise<void> } | null = null;
        if (workspace) {
          for (const uuid of await workspaceJobIds(bin, workspace)) {
            await untrackLogged(deps.tracker, uuid, "kill-workspace");
          }
        } else {
          pre = await preUntrack(deps, input, bin, context.sessionID, id as string);
        }
        const args =
          workspace && !id
            ? ["kill", "--json", "--workspace", workspace, "--force"]
            : ["kill", "--json", (id ?? workspace) as string];
        const result = await runBgrun(bin, args, context.signal);
        if (!result.ok) {
          await pre?.restore();
          await failWithSuggestions(bin, result.stdout, id);
        }
        return { content: renderToolText("kill", parseJson(result.stdout)) };
      },
      input: {
        additionalProperties: false,
        properties: {
          id: { description: "Job ID or name (unless workspace is given).", type: "string" },
          workspace: { description: "Kill every job in this workspace instead.", type: "string" },
        },
        type: "object",
      },
      name: "kill",
    },
    {
      description:
        "Gracefully stop a job: SIGTERM first, escalating to SIGKILL after the grace period. Also marks already-finished jobs as killed to suppress pending restarts.",
      async execute(input, context) {
        const id = str(input["id"]);
        if (!id) throw new Error("missing required argument: id");
        const pre = await preUntrack(deps, input, bin, context.sessionID, id);
        const result = await runBgrun(
          bin,
          ["stop", "--json", id, ...timeoutArgs("--timeout", num(input["timeout_ms"]), 10_000)],
          context.signal,
        );
        if (!result.ok) {
          await pre.restore();
          await failWithSuggestions(bin, result.stdout, id);
        }
        return { content: renderToolText("stop", parseJson(result.stdout)) };
      },
      input: {
        additionalProperties: false,
        properties: {
          id: { description: "Job ID or name.", type: "string" },
          timeout_ms: { description: "Grace period before SIGKILL (default 10000).", type: "number" },
        },
        required: ["id"],
        type: "object",
      },
      name: "stop",
    },
    {
      description: "Stop the job if alive (same as stop), then re-spawn it from its stored definition and return the new record.",
      async execute(input, context) {
        const id = str(input["id"]);
        if (!id) throw new Error("missing required argument: id");
        // Old incarnation is terminated by the respawn: untrack up front, no
        // restore (a failed respawn leaves it dead; the error itself informs).
        await preUntrack(deps, input, bin, context.sessionID, id);
        const result = await runBgrun(
          bin,
          ["restart", "--json", id, ...timeoutArgs("--timeout", num(input["timeout_ms"]), 10_000)],
          context.signal,
        );
        if (!result.ok) await failWithSuggestions(bin, result.stdout, id);
        const record = parseJson(result.stdout) as AnyRecord;
        await trackSpawn(deps, input, context.sessionID, record);
        return { content: renderToolText("restart", record) };
      },
      input: {
        additionalProperties: false,
        properties: {
          id: { description: "Job ID or name.", type: "string" },
          timeout_ms: { description: "Grace period before SIGKILL (default 10000).", type: "number" },
        },
        required: ["id"],
        type: "object",
      },
      name: "restart",
    },
    {
      description:
        "Read the last `lines` (default 50, capped at 1000) of a job's log. With `digest: true` and no `lines`, returns only the summary (total lines, error/warning counts, last error).",
      async execute(input, context) {
        const id = str(input["id"]);
        if (!id) throw new Error("missing required argument: id");
        const digest = bool(input["digest"]);
        const rawLines = num(input["lines"]);
        const lines = rawLines === undefined ? (digest ? 0 : 50) : capLines(rawLines);
        const args = ["tail", "--json", id, "--lines", String(lines)];
        if (digest) args.push("--digest");
        const level = str(input["level"]);
        if (level) args.push("--level", level);
        const stream = str(input["stream"]);
        if (stream) args.push("--stream", stream);
        if (bool(input["strip_ansi"])) args.push("--strip-ansi");
        const filterRegex = str(input["filter_regex"]);
        if (filterRegex) args.push("--filter-regex", filterRegex);
        const result = await runBgrun(bin, args, context.signal);
        if (!result.ok) await failWithSuggestions(bin, result.stdout, id);
        return { content: renderToolText("tail", parseJson(result.stdout)) };
      },
      input: {
        additionalProperties: false,
        properties: {
          digest: { description: "Return an error/warning digest instead of raw lines.", type: "boolean" },
          filter_regex: { description: "Only return lines matching this regex.", type: "string" },
          id: { description: "Job ID or name.", type: "string" },
          level: { description: "Substring filter (e.g. \"error\", \"warn\").", type: "string" },
          lines: { description: "How many lines (default 50, capped at 1000).", type: "number" },
          stream: { description: "Filter by stream: stdout, stderr, or pty.", type: "string" },
          strip_ansi: { description: "Strip ANSI escape codes.", type: "boolean" },
        },
        required: ["id"],
        type: "object",
      },
      name: "tail",
    },
    {
      description:
        "Return only the log lines added since the previous diff call for this job (incremental polling with a per-job cursor).",
      async execute(input, context) {
        const id = str(input["id"]);
        if (!id) throw new Error("missing required argument: id");
        const args = ["diff", "--json", id];
        const lines = num(input["lines"]);
        if (lines !== undefined) args.push("--lines", String(capLines(lines)));
        const stream = str(input["stream"]);
        if (stream) args.push("--stream", stream);
        if (bool(input["strip_ansi"])) args.push("--strip-ansi");
        const filterRegex = str(input["filter_regex"]);
        if (filterRegex) args.push("--filter-regex", filterRegex);
        const result = await runBgrun(bin, args, context.signal);
        if (!result.ok) await failWithSuggestions(bin, result.stdout, id);
        return { content: renderToolText("diff", parseJson(result.stdout)) };
      },
      input: {
        additionalProperties: false,
        properties: {
          filter_regex: { description: "Only return lines matching this regex.", type: "string" },
          id: { description: "Job ID or name.", type: "string" },
          lines: { description: "Cap the number of returned lines.", type: "number" },
          stream: { description: "Filter by stream: stdout, stderr, or pty.", type: "string" },
          strip_ansi: { description: "Strip ANSI escape codes.", type: "boolean" },
        },
        required: ["id"],
        type: "object",
      },
      name: "diff",
    },
    {
      description: "Block until the job becomes ready, reaches a terminal state, or the timeout elapses (default 60000ms).",
      async execute(input, context) {
        const id = str(input["id"]);
        if (!id) throw new Error("missing required argument: id");
        const result = await runBgrun(
          bin,
          ["wait", "--json", id, ...timeoutArgs("--timeout", num(input["timeout_ms"]), 60_000)],
          context.signal,
        );
        if (!result.ok) await failWithSuggestions(bin, result.stdout, id);
        return { content: renderToolText("wait", parseJson(result.stdout)) };
      },
      input: {
        additionalProperties: false,
        properties: {
          id: { description: "Job ID or name.", type: "string" },
          timeout_ms: { description: "Maximum wait (default 60000).", type: "number" },
        },
        required: ["id"],
        type: "object",
      },
      name: "wait",
    },
    {
      description:
        "Drive an interactive job's stdin: start it with pty:true, then send input here. End `data` with \\n for Enter. Control keys are raw bytes: Esc is \\u001b, Up/Down/Left/Right are \\u001b[A/B/C/D, Tab is \\t, Ctrl+C is \\u0003, Ctrl+D is \\u0004. Read the result with bgrun_screen (or bgrun_expect to block until a prompt appears).",
      async execute(input, context) {
        const id = str(input["id"]);
        const data = typeof input["data"] === "string" ? (input["data"] as string) : undefined;
        if (!id) throw new Error("missing required argument: id");
        if (data === undefined) throw new Error("missing required argument: data");
        const result = await runBgrun(bin, ["send", "--json", id, data], context.signal);
        if (!result.ok) await failWithSuggestions(bin, result.stdout, id);
        return { content: renderToolText("send", parseJson(result.stdout)) };
      },
      input: {
        additionalProperties: false,
        properties: {
          data: { description: "Bytes to write to the job's stdin.", type: "string" },
          id: { description: "Job ID or name.", type: "string" },
        },
        required: ["id", "data"],
        type: "object",
      },
      name: "send",
    },
    {
      description: "Get resource stats (CPU %, RSS MB, uptime seconds) for a running job.",
      async execute(input, context) {
        const id = str(input["id"]);
        if (!id) throw new Error("missing required argument: id");
        const result = await runBgrun(bin, ["stats", "--json", id], context.signal);
        if (!result.ok) await failWithSuggestions(bin, result.stdout, id);
        return { content: renderToolText("stats", parseJson(result.stdout)) };
      },
      input: idSchema(),
      name: "stats",
    },
    {
      description: "Start several bgrun.toml jobs at once by name (same as `bgrun run-group`).",
      async execute(input, context) {
        const jobs = Array.isArray(input["jobs"]) ? (input["jobs"] as unknown[]) : undefined;
        if (!jobs || jobs.length === 0) throw new Error("missing required argument: jobs");
        const names = jobs.map(String);
        const result = await runBgrun(bin, ["run-group", "--json", ...names], context.signal);
        if (!result.ok) throw new Error(errorText(result.stdout));
        let data: unknown;
        try {
          data = parseNdjson(result.stdout);
        } catch {
          data = parseJson(result.stdout);
        }
        if (Array.isArray(data)) {
          for (const record of data) {
            await trackSpawn(deps, input, context.sessionID, record as AnyRecord);
          }
        }
        return { content: renderToolText("run_group", data) };
      },
      input: {
        additionalProperties: false,
        properties: {
          jobs: {
            description: "Job names from bgrun.toml.",
            items: { type: "string" },
            type: "array",
          },
        },
        required: ["jobs"],
        type: "object",
      },
      name: "run_group",
    },
    {
      description:
        "Block until `pattern` appears in the job's output (plain substring by default, regex with `is_regex: true`), or the timeout elapses (default 60000ms). Matches interactive prompts immediately — even mid-line ones like `>>> ` that never reach the log file.",
      async execute(input, context) {
        const id = str(input["id"]);
        const pattern = str(input["pattern"]);
        if (!id) throw new Error("missing required argument: id");
        if (!pattern) throw new Error("missing required argument: pattern");
        const args = ["expect", "--json", id, pattern];
        if (bool(input["is_regex"])) args.push("--regex");
        if (bool(input["is_regex"])) args.push("--regex");
        args.push(...timeoutArgs("--timeout", num(input["timeout_ms"]), 60_000));
        const result = await runBgrun(bin, args, context.signal);
        if (!result.ok) await failWithSuggestions(bin, result.stdout, id);
        return { content: renderToolText("expect", parseJson(result.stdout)) };
      },
      input: {
        additionalProperties: false,
        properties: {
          id: { description: "Job ID or name.", type: "string" },
          is_regex: { description: "Treat pattern as a regex (default false).", type: "boolean" },
          pattern: { description: "Substring or regex to wait for.", type: "string" },
          timeout_ms: { description: "Maximum wait (default 60000).", type: "number" },
        },
        required: ["id", "pattern"],
        type: "object",
      },
      name: "expect",
    },
    {
      description:
        "Read the last `lines` (default 20, capped at 1000) from a job's in-memory output buffer without touching the log file. For interactive (PTY) jobs this is how you see REPL output and prompts; pass `strip_ansi: true` to remove color/cursor codes first.",
      async execute(input, context) {
        const id = str(input["id"]);
        if (!id) throw new Error("missing required argument: id");
        const lines = capLines(num(input["lines"]) ?? 20);
        const args = ["screen", "--json", id, "--lines", String(lines)];
        if (bool(input["strip_ansi"])) args.push("--strip-ansi");
        const result = await runBgrun(bin, args, context.signal);
        if (!result.ok) await failWithSuggestions(bin, result.stdout, id);
        return { content: renderToolText("screen", parseJson(result.stdout)) };
      },
      input: {
        additionalProperties: false,
        properties: {
          id: { description: "Job ID or name.", type: "string" },
          lines: { description: "How many rows (default 20, capped at 1000).", type: "number" },
          strip_ansi: { description: "Strip ANSI escape codes.", type: "boolean" },
        },
        required: ["id"],
        type: "object",
      },
      name: "screen",
    },
    {
      description: "Remove all terminal-state (crashed/exited/killed) jobs, optionally scoped to a workspace.",
      async execute(input, context) {
        const args = ["clean", "--json", "--force"];
        const workspace = str(input["workspace"]);
        if (workspace) args.push("--workspace", workspace);
        const result = await runBgrun(bin, args, context.signal);
        if (!result.ok) throw new Error(errorText(result.stdout));
        await deps.tracker?.prune().catch(() => {});
        return { content: renderToolText("clean", parseJson(result.stdout)) };
      },
      input: {
        additionalProperties: false,
        properties: {
          workspace: { description: "Only clean jobs in this workspace.", type: "string" },
        },
        type: "object",
      },
      name: "clean",
    },
    {
      description:
        "Track an already-running job for completion notifications in this session — for jobs started with `notify: false` or from the CLI. Pass `notify: false` to untrack instead.",
      async execute(input, context) {
        const handle = str(input["id"]);
        if (!handle) throw new Error("missing required argument: id");
        const uuid = await resolveJobId(bin, handle);
        if (!uuid) {
          throw new Error(await withSuggestions(bin, "job not found", handle));
        }
        if (input["notify"] === false) {
          await untrackLogged(deps.tracker, uuid, "unwatch");
          return { content: renderToolText("watch", { watching: false, id: uuid }) };
        }
        if (!deps.tracker || deps.tracker.defaultMode === false) {
          throw new Error("notifications are disabled (plugin option \"notify\": false)");
        }
        await deps.tracker
          .track(uuid, {
            createdAt: Date.now(),
            label: handle,
            mode: deps.tracker.defaultMode,
            readyNotified: false,
            sessionID: context.sessionID,
          })
          .catch((error) => {
            throw new Error(`failed to track ${uuid}: ${error}`);
          });
        return {
          content: renderToolText("watch", {
            id: uuid,
            mode: deps.tracker.defaultMode,
            watching: true,
          }),
        };
      },
      input: {
        additionalProperties: false,
        properties: {
          id: { description: "Job ID, name handle, or unique id prefix.", type: "string" },
          notify: { description: "Set false to untrack instead (default true).", type: "boolean" },
        },
        required: ["id"],
        type: "object",
      },
      name: "watch",
    },
  ];
}
