import { describe, expect, test } from "bun:test";
import {
  JobWatcher,
  decideNotification,
  type NotifyMode,
  type PersistedWatcherState,
  type TrackedJob,
  type WatcherDeps,
} from "../lib/notify.ts";

function entry(overrides: Partial<TrackedJob> = {}): TrackedJob {
  return {
    createdAt: Date.now(),
    label: '"web" (abc123)',
    mode: "queue",
    readyNotified: false,
    sessionID: "ses_test",
    ...overrides,
  };
}

describe("decideNotification", () => {
  test("ready notifies once, then stays silent", () => {
    const first = decideNotification("abc123", { name: "web", state: "ready" }, entry());
    expect(first?.done).toBe(false);
    expect(first?.markReady).toBe(true);
    expect(first?.text).toContain("is ready");
    const second = decideNotification(
      "abc123",
      { name: "web", state: "ready" },
      entry({ readyNotified: true }),
    );
    expect(second).toBeNull();
  });

  test("running notifies nothing", () => {
    expect(decideNotification("abc123", { state: "running" }, entry())).toBeNull();
    expect(decideNotification("abc123", { state: "starting" }, entry())).toBeNull();
  });

  test("terminal states notify once with done=true", () => {
    const exited = decideNotification("abc123", { name: "web", state: "exited" }, entry())!;
    expect(exited.done).toBe(true);
    expect(exited.text).toContain("exited");
    const crashed = decideNotification(
      "abc123",
      { name: "web", state: "crashed", exit_code: 1 },
      entry(),
    )!;
    expect(crashed.done).toBe(true);
    expect(crashed.text).toContain("crashed (exit 1)");
    expect(crashed.text).toContain("bgrun_tail");
    const killed = decideNotification("abc123", { state: "killed" }, entry())!;
    expect(killed.done).toBe(true);
    expect(killed.text).toContain("was killed");
  });

  test("unnamed jobs use the short id as label", () => {
    const decided = decideNotification("abcdef123456", { state: "exited" }, entry())!;
    expect(decided.text).toContain("abcdef12");
  });
});

interface Harness {
  jobs: Map<string, Record<string, any>>;
  notified: Array<{ sessionID: string; text: string; mode: NotifyMode }>;
  saved: Array<PersistedWatcherState>;
  notifyFails: boolean;
  watcher: JobWatcher;
}

function harness(): Harness {
  const h = {} as Harness;
  h.jobs = new Map();
  h.notified = [];
  h.saved = [];
  h.notifyFails = false;
  const deps: WatcherDeps = {
    bin: "bgrun",
    listJobs: async () => new Map(h.jobs),
    load: async () => ({ tracked: {}, receipts: {} }),
    notify: async (sessionID, text, mode) => {
      if (h.notifyFails) throw new Error("session gone");
      h.notified.push({ mode, sessionID, text });
    },
    pollMs: 50,
    save: async (state) => {
      h.saved.push(structuredClone(state));
    },
  };
  h.watcher = new JobWatcher(deps);
  return h;
}

describe("JobWatcher", () => {
  test("ready then exit notifies twice, then untracks", async () => {
    const h = harness();
    await h.watcher.restore();
    await h.watcher.track("abc", entry());
    h.jobs.set("abc", { id: "abc", name: "web", state: "running" });
    await h.watcher.poll();
    expect(h.notified).toHaveLength(0);

    h.jobs.set("abc", { id: "abc", name: "web", state: "ready" });
    await h.watcher.poll();
    expect(h.notified).toHaveLength(1);
    expect(h.watcher.size).toBe(1);

    // Second ready poll stays silent (already notified).
    await h.watcher.poll();
    expect(h.notified).toHaveLength(1);

    h.jobs.set("abc", { id: "abc", name: "web", state: "exited" });
    await h.watcher.poll();
    expect(h.notified).toHaveLength(2);
    expect(h.watcher.size).toBe(0);
  });

  test("vanished jobs drop silently", async () => {
    const h = harness();
    await h.watcher.restore();
    await h.watcher.track("gone", entry());
    await h.watcher.poll();
    expect(h.watcher.size).toBe(0);
    expect(h.notified).toHaveLength(0);
  });

  test("dead sessions untrack instead of retrying forever", async () => {
    const h = harness();
    h.notifyFails = true;
    await h.watcher.restore();
    await h.watcher.track("abc", entry());
    h.jobs.set("abc", { id: "abc", state: "exited" });
    await h.watcher.poll();
    expect(h.watcher.size).toBe(0);
  });

  test("failed ready delivery marks notified so reloads never duplicate", async () => {
    const h = harness();
    h.notifyFails = true;
    await h.watcher.restore();
    await h.watcher.track("abc", entry());
    h.jobs.set("abc", { id: "abc", state: "ready" });
    await h.watcher.poll();
    // Entry retained (job still running) but marked: the next poll must not
    // attempt delivery again.
    expect(h.watcher.size).toBe(1);
    await h.watcher.poll();
    expect(h.notified).toHaveLength(0);
  });

  test("empty tracker never lists", async () => {
    const h = harness();
    let listed = false;
    (h.watcher as any).deps.listJobs = async () => {
      listed = true;
      return new Map();
    };
    await h.watcher.poll();
    expect(listed).toBe(false);
  });

  test("restore reloads persisted tracking", async () => {
    const h = harness();
    (h.watcher as any).deps.load = async () => ({ tracked: { abc: entry() }, receipts: {} });
    await h.watcher.restore();
    expect(h.watcher.size).toBe(1);
  });

  test("two instances sharing storage deliver exactly once", async () => {
    // The production failure: OpenCode loads one plugin instance per
    // location, and every instance restores the same tracked map. Without
    // receipts each instance queues its own copy of every notification.
    const shared: { state: PersistedWatcherState } = {
      state: { tracked: { abc: entry() }, receipts: {} },
    };
    const notified: string[] = [];
    const makeDeps = (): WatcherDeps => ({
      bin: "bgrun",
      listJobs: async () =>
        new Map([["abc", { id: "abc", name: "web", state: "exited" }]]),
      load: async () => structuredClone(shared.state),
      notify: async (_sessionID, text) => {
        notified.push(text);
      },
      pollMs: 50,
      save: async (state) => {
        shared.state = structuredClone(state);
      },
    });
    const a = new JobWatcher(makeDeps());
    const b = new JobWatcher(makeDeps());
    await a.restore();
    await b.restore();
    // Interleaved polls, as two 5s-interval instances would run them.
    await a.poll();
    await b.poll();
    await a.poll();
    expect(notified).toHaveLength(1);
    expect(shared.state.tracked).toEqual({});
    expect(Object.keys(shared.state.receipts)).toEqual(["notified.abc.terminal"]);
  });

  test("untrack and prune drop entries and persist", async () => {
    const h = harness();
    await h.watcher.restore();
    await h.watcher.track("a", entry());
    await h.watcher.track("b", entry());
    await h.watcher.untrack("a");
    expect(h.watcher.size).toBe(1);
    h.jobs.set("b", { id: "b", state: "running" });
    await h.watcher.prune();
    expect(h.watcher.size).toBe(1);
    h.jobs.clear();
    await h.watcher.prune();
    expect(h.watcher.size).toBe(0);
    expect(h.saved.length).toBeGreaterThan(0);
  });
});
