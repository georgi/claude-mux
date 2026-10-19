// The tree (UI design 4.1, left): plans, then each plan's agents nested by
// parent, then loose terminals. A pure function of the store's view.

import type { AgentView, StateView, TaskView, TermId } from "./api/types";
import { agentStatus, planStatus, type Status, TERMINAL_STATUS } from "./lib/status";

export interface Row {
  key: string;
  depth: number;
  glyph: string;
  status: Status;
  label: string;
  /** Short trailing info: task state, PR number. */
  detail: string;
  agent?: string;
  term?: TermId;
  /** Muted: finished or stopped. */
  dim: boolean;
}

/** The state glyph the TUI uses. `spin` is the current spinner frame. */
export function agentGlyph(a: AgentView, spin = "⠹"): string {
  if (typeof a.life === "object") return a.life.exited.code === 0 || a.report?.status === "success" ? "✓" : "✗";
  if (a.life === "stopped") return "■";
  if (a.life === "starting") return "○";
  if (a.stalled) return "!";
  switch (a.activity) {
    case "working":
      return spin;
    case "needs_permission":
      return "⚑";
    case "idle":
      return "◆";
    case "unknown":
      return "?";
  }
}

function taskDetail(t: TaskView | undefined): string {
  if (!t) return "";
  const pr = t.pr ? ` #${t.pr.number}` : "";
  return `${t.state.replace("_", " ")}${pr}`;
}

export function buildTree(v: StateView, spin?: string): Row[] {
  const rows: Row[] = [];
  const children = new Map<string | null, AgentView[]>();
  for (const a of v.agents) {
    const list = children.get(a.parent) ?? [];
    list.push(a);
    children.set(a.parent, list);
  }
  for (const p of v.plans) {
    rows.push({
      key: p.id,
      depth: 0,
      glyph: p.state === "done" ? "✓" : p.state === "accepted" ? "◆" : "○",
      status: planStatus(p.state, p.paused),
      label: p.title,
      detail: p.paused ? "paused" : p.state,
      dim: p.state === "done" || p.state === "cancelled",
    });
    const order = new Map(p.agents.map((id, i) => [id, i]));
    const byOrder = (a: AgentView, b: AgentView) => (order.get(a.id) ?? 1e9) - (order.get(b.id) ?? 1e9);
    const walk = (a: AgentView, depth: number) => {
      const t = a.task ? v.tasks.find((x) => x.plan === p.id && x.id === a.task) : undefined;
      const name = a.level === "sub" ? a.path.slice(a.path.indexOf("/") + 1) : a.path;
      rows.push({
        key: a.id,
        depth,
        glyph: agentGlyph(a, spin),
        status: agentStatus(a),
        label: t && a.level === "task" ? `${name} ${t.title}` : name,
        detail: a.level === "task" ? taskDetail(t) : "",
        agent: a.id,
        term: a.term ?? undefined,
        dim: typeof a.life === "object" || a.life === "stopped",
      });
      for (const c of (children.get(a.id) ?? []).sort(byOrder)) walk(c, depth + 1);
    };
    const roots = v.agents.filter(
      (a) => a.id.startsWith(`${p.id}/`) && (a.parent === null || !v.agents.some((x) => x.id === a.parent)),
    );
    for (const a of roots.sort(byOrder)) walk(a, 1);
  }
  for (const id of v.loose) {
    const t = v.terminals.find((x) => x.id === id);
    if (!t) continue;
    const kind = typeof t.kind === "object" ? t.kind.command : t.kind;
    rows.push({
      key: `term-${id}`,
      depth: 0,
      glyph: "▸",
      status: TERMINAL_STATUS,
      label: t.title ?? kind,
      detail: "",
      term: id,
      dim: false,
    });
  }
  return rows;
}
