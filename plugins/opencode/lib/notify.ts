/// Job-completion notifier: the capability MCP structurally cannot offer.
///
/// MCP tools are stateless and session-blind. The plugin instead records
/// which session started which job, polls the daemon, and writes the outcome
/// back into that exact session — so `run` returns immediately and the agent
/// is woken with results instead of blocking in `wait` or polling `diff`.
///
/// Design notes:
/// - Tracking is per job id, persisted in `ctx.storage` so it survives
///   plugin reloads (hot reload during dev, OpenCode restarts).
/// - The poll loop only hits the daemon while tracked jobs exist.
/// - Notify failures (deleted session) untrack rather than retry forever.
/// - `JobWatcher` takes injected `listJobs`/`notify`/`load`/`save` so the
///   whole cycle is unit-testable without a daemon (see tests/notify.test.ts).

export type NotifyMode = "queue" | "synthetic";

export interface TrackedJob {
  /** Originating OpenCode session — completion is reported here. */
  sessionID: string;
  /** Human label (job name or short id) used in messages. */
  label: string;
  /** Delivery captured at track time (global default, per-call override). */
  mode: NotifyMode;
  /** Ready already reported; terminal still pends. */
  readyNotified: boolean;
  createdAt: number;
}

/// Persisted watcher state (single storage key). Receipts make delivery
/// exactly-once across plugin INSTANCES: OpenCode loads one plugin instance
/// per location, so N locations mean N watchers sharing this blob — without
/// receipts every instance would queue its own copy of each notification.
export interface PersistedWatcherState {
  tracked: Record<string, TrackedJob>;
  /** `notified.<jobId>.<ready|terminal>` → epoch ms of delivery. */
  receipts: Record<string, number>;
}

const RECEIPT_TTL_MS = 24 * 60 * 60 * 1000;

export interface WatcherDeps {
  bin: string;
  pollMs: number;
  listJobs: () => Promise<Map<string, Record<string, any>>>;
  notify: (sessionID: string, text: string, mode: NotifyMode) => Promise<void>;
  load: () => Promise<PersistedWatcherState>;
  save: (state: PersistedWatcherState) => Promise<void>;
}

const TERMINAL_STATES = new Set(["exited", "crashed", "killed"]);

export function shortLabel(record: Record<string, any>, id: string): string {
  const name = typeof record["name"] === "string" ? record["name"] : "";
  return name.length > 0 ? `"${name}" (${id.slice(0, 8)})` : id.slice(0, 8);
}

/// Pure decision: given a daemon record and tracking entry, report what (if
/// anything) the session should hear. Returns the message plus whether the
/// job is done (untrack) or still running (keep, marking ready reported).
export function decideNotification(
  id: string,
  record: Record<string, any>,
  entry: TrackedJob,
): { text: string; done: boolean; markReady?: boolean } | null {
  const state = typeof record["state"] === "string" ? record["state"] : "";
  const label = shortLabel(record, id);
  const exitCode = record["exit_code"];

  if (state === "ready" && !entry.readyNotified) {
    return {
      done: false,
      markReady: true,
      text: `[bgrun] job ${label} is ready. Use bgrun_tail / bgrun_diff for logs, bgrun_kill to stop it.`,
    };
  }
  if (TERMINAL_STATES.has(state)) {
    const code = typeof exitCode === "number" ? ` (exit ${exitCode})` : "";
    const detail =
      state === "crashed"
        ? ` Use bgrun_tail with level "error" on ${id.slice(0, 8)} for details.`
        : state === "exited"
          ? ` Use bgrun_diff on ${id.slice(0, 8)} for any final output.`
          : "";
    const verb = state === "exited" ? "exited" : state === "crashed" ? `crashed${code}` : "was killed";
    return { done: true, text: `[bgrun] job ${label} ${verb}.${detail}` };
  }
  return null;
}

export class JobWatcher {
  private tracked = new Map<string, TrackedJob>();
  private receipts = new Map<string, number>();
  private timer: ReturnType<typeof setInterval> | undefined;
  private loaded = false;
  private readonly deps: WatcherDeps;

  constructor(deps: WatcherDeps) {
    this.deps = deps;
  }

  /** Restore persisted tracking (call once from setup). */
  async restore(): Promise<void> {
    try {
      const saved = await this.deps.load();
      // Migrate the pre-receipt shape (bare tracked map).
      const state: PersistedWatcherState =
        saved && typeof saved === "object" && "tracked" in saved
          ? (saved as PersistedWatcherState)
          : { tracked: (saved ?? {}) as Record<string, TrackedJob>, receipts: {} };
      for (const [id, entry] of Object.entries(state.tracked ?? {})) {
        if (entry && typeof entry.sessionID === "string") this.tracked.set(id, entry);
      }
      for (const [key, at] of Object.entries(state.receipts ?? {})) {
        if (typeof at === "number") this.receipts.set(key, at);
      }
    } catch {
      // Corrupt storage must never break plugin setup.
    }
    this.loaded = true;
  }

  start(): void {
    if (this.timer) return;
    this.timer = setInterval(() => {
      void this.poll().catch((error) => console.error(`[bgrun] watcher poll failed: ${error}`));
    }, this.deps.pollMs);
    // Unref so the interval never holds the host process open on its own.
    (this.timer as unknown as { unref?: () => void }).unref?.();
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
  }

  get size(): number {
    return this.tracked.size;
  }

  async track(id: string, entry: TrackedJob): Promise<void> {
    this.tracked.set(id, entry);
    await this.persist();
  }

  async untrack(id: string): Promise<void> {
    if (this.tracked.delete(id)) await this.persist();
  }

  /** Drop tracked ids that no longer exist (after `clean`, external kills). */
  async prune(): Promise<void> {
    if (this.tracked.size === 0) return;
    let jobs: Map<string, Record<string, any>>;
    try {
      jobs = await this.deps.listJobs();
    } catch {
      return;
    }
    let changed = false;
    for (const id of this.tracked.keys()) {
      if (!jobs.has(id)) {
        this.tracked.delete(id);
        changed = true;
      }
    }
    if (changed) await this.persist();
  }

  /** One poll cycle: notify sessions exactly once per outcome, even with
   * several watcher instances sharing storage. The claim sequence per
   * outcome is receipt-check → jitter → re-check → claim → persist →
   * deliver, so two instances racing the same outcome collapse to one
   * delivery. Public for tests. */
  async poll(): Promise<void> {
    if (this.tracked.size === 0) return;
    let jobs: Map<string, Record<string, any>>;
    try {
      jobs = await this.deps.listJobs();
    } catch (error) {
      console.error(`[bgrun] watcher list failed: ${error}`);
      return;
    }
    this.refreshReceipts();
    let changed = false;
    for (const [id, entry] of [...this.tracked]) {
      const record = jobs.get(id);
      if (!record) {
        // Cleaned or otherwise gone — drop silently.
        this.tracked.delete(id);
        changed = true;
        continue;
      }
      const decision = decideNotification(id, record, entry);
      if (!decision) continue;
      const kind = decision.done ? "terminal" : "ready";
      const receipt = `notified.${id}.${kind}`;
      if (this.receipts.has(receipt)) {
        // Another instance (or a pre-reload self) already delivered this.
        if (decision.done) this.tracked.delete(id);
        else if (decision.markReady) entry.readyNotified = true;
        changed = true;
        continue;
      }
      // Jitter before claiming: simultaneous instances spread their claims.
      await new Promise((resolve) => setTimeout(resolve, Math.random() * 300));
      await this.refreshReceipts();
      if (this.receipts.has(receipt)) {
        if (decision.done) this.tracked.delete(id);
        else if (decision.markReady) entry.readyNotified = true;
        changed = true;
        continue;
      }
      // Commit claim BEFORE delivering: a reload between delivery and
      // persist must never resurrect a delivered notification.
      this.receipts.set(receipt, Date.now());
      if (decision.done) {
        this.tracked.delete(id);
      } else if (decision.markReady) {
        entry.readyNotified = true;
      }
      changed = true;
      await this.persist();
      try {
        await this.deps.notify(entry.sessionID, decision.text, entry.mode);
      } catch (error) {
        // Delivery failed (blocked/deleted session): the outcome stays
        // claimed — redelivering into a void helps nobody.
        console.error(`[bgrun] notify ${entry.sessionID} about job ${id} failed: ${error}`);
      }
    }
    if (changed) await this.persist();
  }

  /** Re-read receipts (another instance may have claimed) and drop expired. */
  private async refreshReceipts(): Promise<void> {
    try {
      const saved = await this.deps.load();
      const receipts =
        saved && typeof saved === "object" && "receipts" in saved
          ? (saved as PersistedWatcherState).receipts ?? {}
          : {};
      const now = Date.now();
      for (const [key, at] of Object.entries(receipts)) {
        if (typeof at === "number" && now - at < RECEIPT_TTL_MS) this.receipts.set(key, at);
      }
    } catch {
      // Storage hiccup: proceed with in-memory receipts.
    }
    const now = Date.now();
    for (const [key, at] of [...this.receipts]) {
      if (now - at >= RECEIPT_TTL_MS) this.receipts.delete(key);
    }
  }

  private async persist(): Promise<void> {
    if (!this.loaded) return;
    try {
      await this.deps.save({
        receipts: Object.fromEntries(this.receipts),
        tracked: Object.fromEntries(this.tracked),
      });
    } catch (error) {
      console.error(`[bgrun] watcher persist failed: ${error}`);
    }
  }
}
