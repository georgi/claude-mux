// The one store (architecture 11.2): fed by the stream's `state` and
// `deltas` messages, plus terminal sizes, presence and the connection's own
// status. Views are pure renders of it. `reduce` is pure, so the tests drive
// it with plain messages.

import type {
  AgentId,
  ClientId,
  DeltaView,
  ErrorCode,
  PresenceEntry,
  Scope,
  ServerInfo,
  ServerMsg,
  StateView,
  TermId,
} from "./api/types";

export type ConnStatus = "connecting" | "open" | "closed" | "unauthorized";

export interface TermSize {
  rows: number;
  cols: number;
  owner: ClientId | null;
}

export interface Model {
  conn: ConnStatus;
  server: ServerInfo | null;
  clientId: ClientId | null;
  scope: Scope | null;
  /** `null` until the first `state` message. */
  view: StateView | null;
  sizes: Record<TermId, TermSize>;
  exits: Record<TermId, { code: number; signal: string | null }>;
  presence: PresenceEntry[];
  error: { code: ErrorCode; message: string } | null;
}

export type Event = ServerMsg | { type: "conn"; status: ConnStatus };

export const initial: Model = {
  conn: "connecting",
  server: null,
  clientId: null,
  scope: null,
  view: null,
  sizes: {},
  exits: {},
  presence: [],
  error: null,
};

function upsert<T>(list: T[], item: T, same: (x: T) => boolean): T[] {
  const at = list.findIndex(same);
  if (at < 0) return [...list, item];
  const out = list.slice();
  out[at] = item;
  return out;
}

/** One delta, as `StateView::apply` in cmux-proto: whole objects replace. */
export function applyDelta(v: StateView, d: DeltaView): StateView {
  const rev = Math.max(v.rev, d.rev);
  const c = d.change;
  switch (c.kind) {
    case "plan":
      return { ...v, rev, plans: upsert(v.plans, c.value, (x) => x.id === c.value.id) };
    case "task":
      return {
        ...v,
        rev,
        tasks: upsert(v.tasks, c.value, (x) => x.plan === c.value.plan && x.id === c.value.id),
      };
    case "agent":
      return { ...v, rev, agents: upsert(v.agents, c.value, (x) => x.id === c.value.id) };
    case "terminal": {
      const t = c.value;
      const terminals = upsert(v.terminals, t, (x) => x.id === t.id);
      // Loose terminals have no agent; the order list follows.
      const loose = t.agent === null && !v.loose.includes(t.id) ? [...v.loose, t.id] : v.loose;
      return { ...v, rev, terminals, loose };
    }
    case "item":
      return { ...v, rev, items: upsert(v.items, c.value, (x) => x.id === c.value.id) };
    case "removed": {
      const o = c.value;
      switch (o.kind) {
        case "plan":
          return {
            ...v,
            rev,
            plans: v.plans.filter((p) => p.id !== o.plan),
            tasks: v.tasks.filter((t) => t.plan !== o.plan),
            agents: v.agents.filter((a) => planOf(a.id) !== o.plan),
          };
        case "task":
          return { ...v, rev, tasks: v.tasks.filter((t) => !(t.plan === o.plan && t.id === o.task)) };
        case "agent":
          return { ...v, rev, agents: v.agents.filter((a) => a.id !== o.agent) };
        case "terminal":
          return {
            ...v,
            rev,
            terminals: v.terminals.filter((t) => t.id !== o.term),
            loose: v.loose.filter((t) => t !== o.term),
          };
        case "item":
          return { ...v, rev, items: v.items.filter((i) => i.id !== o.item) };
      }
    }
  }
}

/** The plan part of an agent id (`P-…/executor` → `P-…`). */
export function planOf(agent: AgentId): string {
  const at = agent.indexOf("/");
  return at < 0 ? agent : agent.slice(0, at);
}

export function reduce(m: Model, e: Event): Model {
  switch (e.type) {
    case "conn":
      return { ...m, conn: e.status };
    case "welcome":
      return { ...m, conn: "open", server: e.server, clientId: e.client_id, scope: e.scope, error: null };
    case "state":
      return { ...m, view: { ...e.snapshot, rev: Math.max(e.rev, e.snapshot.rev) } };
    case "deltas": {
      // Deltas only follow a snapshot; before one there is nothing to patch.
      if (!m.view) return m;
      const view = e.items.reduce(applyDelta, m.view);
      return { ...m, view: { ...view, rev: Math.max(view.rev, e.rev) } };
    }
    case "size":
      return { ...m, sizes: { ...m.sizes, [e.term]: { rows: e.rows, cols: e.cols, owner: e.owner } } };
    case "exit":
      return { ...m, exits: { ...m.exits, [e.term]: { code: e.code, signal: e.signal } } };
    case "presence":
      return { ...m, presence: e.clients };
    case "error":
      return {
        ...m,
        conn: e.code === "unauthorized" ? "unauthorized" : m.conn,
        error: { code: e.code, message: e.message },
      };
    case "chat":
    case "pong":
      return m;
  }
}

/** A tiny observable around `reduce`. */
export class Store {
  private model: Model;
  private listeners = new Set<() => void>();

  constructor(model: Model = initial) {
    this.model = model;
  }

  get(): Model {
    return this.model;
  }

  dispatch(e: Event): void {
    const next = reduce(this.model, e);
    if (next === this.model) return;
    this.model = next;
    for (const l of this.listeners) l();
  }

  subscribe(l: () => void): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }
}
