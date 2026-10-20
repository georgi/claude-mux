import { describe, expect, it } from "vitest";
import type { DeltaView } from "./api/types";
import { agent, entry, item, PLAN, plan, state, task, terminal } from "./fixtures";
import { applyDelta, initial, reduce, Store } from "./store";

const d = (rev: number, change: DeltaView["change"]): DeltaView => ({ rev, change });

describe("applyDelta", () => {
  it("inserts new objects and replaces existing ones whole", () => {
    let v = state({ plans: [plan()], tasks: [task("T-0001")] });
    v = applyDelta(v, d(2, { kind: "task", value: task("T-0001", { state: "in_progress" }) }));
    v = applyDelta(v, d(2, { kind: "task", value: task("T-0002") }));
    expect(v.rev).toBe(2);
    expect(v.tasks.map((t) => [t.id, t.state])).toEqual([
      ["T-0001", "in_progress"],
      ["T-0002", "todo"],
    ]);
    v = applyDelta(v, d(3, { kind: "plan", value: plan({ title: "renamed" }) }));
    expect(v.plans).toHaveLength(1);
    expect(v.plans[0].title).toBe("renamed");
    v = applyDelta(v, d(3, { kind: "agent", value: agent("executor") }));
    v = applyDelta(v, d(4, { kind: "agent", value: agent("executor", { activity: "idle" }) }));
    expect(v.agents.map((a) => a.activity)).toEqual(["idle"]);
    v = applyDelta(v, d(4, { kind: "item", value: item(1) }));
    v = applyDelta(v, d(5, { kind: "item", value: item(1, { status: "dismissed" }) }));
    expect(v.items.map((i) => i.status)).toEqual(["dismissed"]);
  });

  it("matches tasks by plan and id", () => {
    let v = state({ tasks: [task("T-0001")] });
    v = applyDelta(v, d(2, { kind: "task", value: task("T-0001", { plan: "P-20261018-other" }) }));
    expect(v.tasks).toHaveLength(2);
  });

  it("tracks loose terminals in creation order", () => {
    let v = state();
    v = applyDelta(v, d(2, { kind: "terminal", value: terminal(3) }));
    v = applyDelta(v, d(2, { kind: "terminal", value: terminal(1, { agent: `${PLAN}/executor` }) }));
    v = applyDelta(v, d(3, { kind: "terminal", value: terminal(2) }));
    v = applyDelta(v, d(4, { kind: "terminal", value: terminal(3, { title: "zsh" }) }));
    expect(v.loose).toEqual([3, 2]);
    expect(v.terminals.map((t) => t.id)).toEqual([3, 1, 2]);
    v = applyDelta(v, d(5, { kind: "removed", value: { kind: "terminal", term: 3 } }));
    expect(v.loose).toEqual([2]);
    expect(v.terminals.map((t) => t.id)).toEqual([1, 2]);
  });

  it("removes a plan with its tasks and agents", () => {
    const other = "P-20261018-other";
    let v = state({
      plans: [plan(), plan({ id: other })],
      tasks: [task("T-0001"), task("T-0001", { plan: other })],
      agents: [agent("executor"), agent("T-0001"), agent("executor", { id: `${other}/executor` })],
      items: [item(1)],
    });
    v = applyDelta(v, d(9, { kind: "removed", value: { kind: "plan", plan: PLAN } }));
    expect(v.plans.map((p) => p.id)).toEqual([other]);
    expect(v.tasks.map((t) => t.plan)).toEqual([other]);
    expect(v.agents.map((a) => a.id)).toEqual([`${other}/executor`]);
    expect(v.items).toHaveLength(1);
    expect(v.rev).toBe(9);
  });

  it("removes single tasks, agents and items", () => {
    let v = state({ tasks: [task("T-0001"), task("T-0002")], agents: [agent("T-0001")], items: [item(1), item(2)] });
    v = applyDelta(v, d(2, { kind: "removed", value: { kind: "task", plan: PLAN, task: "T-0001" } }));
    v = applyDelta(v, d(2, { kind: "removed", value: { kind: "agent", agent: `${PLAN}/T-0001` } }));
    v = applyDelta(v, d(2, { kind: "removed", value: { kind: "item", item: 2 } }));
    expect(v.tasks.map((t) => t.id)).toEqual(["T-0002"]);
    expect(v.agents).toEqual([]);
    expect(v.items.map((i) => i.id)).toEqual([1]);
  });

  it("never moves rev backwards and never mutates its input", () => {
    const v = state({ rev: 10, tasks: [task("T-0001")] });
    const frozen = JSON.stringify(v);
    const w = applyDelta(v, d(4, { kind: "task", value: task("T-0001", { state: "merged" }) }));
    expect(w.rev).toBe(10);
    expect(w.tasks[0].state).toBe("merged");
    expect(JSON.stringify(v)).toBe(frozen);
  });
});

describe("reduce", () => {
  const welcome = {
    type: "welcome",
    rev: 4,
    server: { version: "0.1.0", name: "vps" },
    client_id: "c-7",
    scope: "full",
  } as const;

  it("takes the welcome, then a snapshot, then deltas", () => {
    let m = reduce(initial, welcome);
    expect(m.conn).toBe("open");
    expect(m.clientId).toBe("c-7");
    // Deltas before any snapshot have nothing to patch.
    expect(reduce(m, { type: "deltas", rev: 5, items: [] })).toBe(m);
    m = reduce(m, { type: "state", rev: 4, snapshot: state({ rev: 4, plans: [plan()] }) });
    expect(m.view?.rev).toBe(4);
    m = reduce(m, {
      type: "deltas",
      rev: 6,
      items: [d(5, { kind: "task", value: task("T-0001") }), d(6, { kind: "agent", value: agent("T-0001") })],
    });
    expect(m.view?.rev).toBe(6);
    expect(m.view?.tasks).toHaveLength(1);
    expect(m.view?.agents).toHaveLength(1);
    // An empty batch after a reconnect still advances rev.
    m = reduce(m, { type: "deltas", rev: 8, items: [] });
    expect(m.view?.rev).toBe(8);
  });

  it("keeps the view across a reconnect so deltas resume it", () => {
    let m = reduce(initial, welcome);
    m = reduce(m, { type: "state", rev: 4, snapshot: state({ rev: 4, tasks: [task("T-0001")] }) });
    m = reduce(m, { type: "conn", status: "closed" });
    expect(m.conn).toBe("closed");
    expect(m.view?.tasks).toHaveLength(1);
    m = reduce(m, { ...welcome, rev: 5 });
    m = reduce(m, {
      type: "deltas",
      rev: 5,
      items: [d(5, { kind: "task", value: task("T-0001", { state: "merged" }) })],
    });
    expect(m.view?.tasks[0].state).toBe("merged");
    // A fresh snapshot (since_rev too old) replaces everything.
    m = reduce(m, { type: "state", rev: 20, snapshot: state({ rev: 20 }) });
    expect(m.view?.tasks).toEqual([]);
    expect(m.view?.rev).toBe(20);
  });

  it("records sizes, exits, presence and errors", () => {
    let m = reduce(initial, { type: "size", term: 1, rows: 30, cols: 100, owner: "c-1" });
    expect(m.sizes[1]).toEqual({ rows: 30, cols: 100, owner: "c-1" });
    m = reduce(m, { type: "exit", term: 1, code: 0, signal: null });
    expect(m.exits[1]).toEqual({ code: 0, signal: null });
    m = reduce(m, { type: "presence", clients: [{ id: "c-1", kind: "tui", device: null }] });
    expect(m.presence).toHaveLength(1);
    m = reduce(m, { type: "error", code: "forbidden", message: "read-only" });
    expect(m.error?.code).toBe("forbidden");
    expect(m.conn).toBe("connecting");
    m = reduce(m, { type: "error", code: "unauthorized", message: "no" });
    expect(m.conn).toBe("unauthorized");
    expect(reduce(m, { type: "pong", nonce: 1 })).toBe(m);
  });

  it("notifies subscribers only on change", () => {
    const s = new Store();
    let n = 0;
    const off = s.subscribe(() => n++);
    s.dispatch({ type: "pong", nonce: 0 });
    s.dispatch({ type: "conn", status: "open" });
    off();
    s.dispatch({ type: "conn", status: "closed" });
    expect(n).toBe(1);
    expect(s.get().conn).toBe("closed");
  });
});

describe("chat", () => {
  it("keeps each agent's entries, and an empty first batch marks it loaded", () => {
    let m = reduce(initial, { type: "chat", agent: "a", entries: [] });
    expect(m.chats).toEqual({ a: [] });
    expect(reduce(m, { type: "chat", agent: "a", entries: [] })).toBe(m);
    m = reduce(m, { type: "chat", agent: "a", entries: [entry("1")] });
    m = reduce(m, { type: "chat", agent: "b", entries: [entry("9")] });
    m = reduce(m, { type: "chat", agent: "a", entries: [entry("1"), entry("2")] });
    expect(m.chats.a.map((e) => e.uuid)).toEqual(["1", "2"]);
    expect(m.chats.b.map((e) => e.uuid)).toEqual(["9"]);
  });
});
