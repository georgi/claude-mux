// The tree (UI design 4.1, left): plans with their executor first, then
// their tasks in plan order, sub-agents under the task agent; then free
// sessions (loose terminals and agents of no plan). Pure functions of the
// store's view, so the tests drive them with fixtures.

import type { AgentView, PlanView, StateView, TaskView, TermId } from "./api/types";
import { pending } from "./needs";
import { AGENT_STATUS, agentStatus, planStatus, type Status, TERMINAL_STATUS, taskStatus } from "./lib/status";

export type NodeKind = "plan" | "task" | "agent" | "term" | "group";

export interface TreeNode {
  key: string;
  kind: NodeKind;
  status: Status;
  label: string;
  /** Short trailing info: task state, PR number, merged/total. */
  detail: string;
  /** The plan's state badge. */
  badge?: string;
  /** What the filter matches: path, task id, title. */
  match: string[];
  plan?: PlanView;
  agent?: AgentView;
  task?: TaskView;
  term?: TermId;
  /** The directory the row works in. */
  cwd?: string;
  /** Muted: finished or stopped. */
  dim: boolean;
  /** Exited without a report: a muted strike. */
  struck: boolean;
  stalled: boolean;
  /** Pending needs-you items from this row's agent (or plan). */
  needs: number;
  children: TreeNode[];
}

/** The key of the free-sessions group. */
export const SESSIONS = "sessions";

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

function taskDetail(t: TaskView): string {
  const pr = t.pr ? ` #${t.pr.number}` : "";
  return `${t.state.replace("_", " ")}${pr}`;
}

/** Exited without a report of its own (the server's implicit one counts as none). */
export function exitedWithoutReport(a: AgentView): boolean {
  return typeof a.life === "object" && (a.report === null || a.report.implicit);
}

export function buildTree(v: StateView): TreeNode[] {
  const needs = new Map<string, number>();
  for (const it of pending(v)) if (it.from) needs.set(it.from, (needs.get(it.from) ?? 0) + 1);
  const children = new Map<string, AgentView[]>();
  for (const a of v.agents)
    if (a.parent) {
      const list = children.get(a.parent) ?? [];
      list.push(a);
      children.set(a.parent, list);
    }
  const placed = new Set<string>();

  const agentNode = (a: AgentView, t: TaskView | undefined, byOrder: (x: AgentView, y: AgentView) => number): TreeNode => {
    placed.add(a.id);
    const name = a.level === "sub" ? a.path.slice(a.path.indexOf("/") + 1) : a.path;
    return {
      key: a.id,
      kind: t ? "task" : "agent",
      status: agentStatus(a),
      label: t ? `${t.id} ${t.title}` : name,
      detail: t ? taskDetail(t) : "",
      match: t ? [a.path, t.id, t.title] : [a.path],
      agent: a,
      task: t,
      term: a.term ?? undefined,
      dim: typeof a.life === "object" || a.life === "stopped",
      struck: exitedWithoutReport(a),
      stalled: a.stalled && a.life !== "stopped" && typeof a.life !== "object",
      needs: needs.get(a.id) ?? 0,
      children: [...(children.get(a.id) ?? [])].sort(byOrder).map((c) => agentNode(c, undefined, byOrder)),
    };
  };

  const nodes: TreeNode[] = [];
  for (const p of v.plans) {
    const order = new Map(p.agents.map((id, i) => [id, i]));
    const byOrder = (a: AgentView, b: AgentView) => (order.get(a.id) ?? 1e9) - (order.get(b.id) ?? 1e9);
    const mine = v.agents.filter((a) => a.id.startsWith(`${p.id}/`));
    const tasks = p.tasks.flatMap((id) => v.tasks.filter((t) => t.plan === p.id && t.id === id));
    const taskAgent = (t: TaskView) =>
      mine.find((a) => a.id === t.agent) ?? mine.find((a) => a.level === "task" && a.task === t.id);
    const taken = new Set(tasks.map(taskAgent).flatMap((a) => (a ? [a.id] : [])));
    const kids: TreeNode[] = [];
    // The executor (and any other agent outside a task) first, its own task agents left out.
    for (const a of mine
      .filter((a) => !taken.has(a.id) && (a.parent === null || !mine.some((x) => x.id === a.parent)))
      .sort(byOrder)) {
      const n = agentNode(a, undefined, byOrder);
      n.children = n.children.filter((c) => !taken.has(c.key));
      kids.push(n);
    }
    for (const t of tasks) {
      const a = taskAgent(t);
      if (a) {
        kids.push(agentNode(a, t, byOrder));
        continue;
      }
      kids.push({
        key: `${p.id}#${t.id}`,
        kind: "task",
        status: taskStatus(t.state),
        label: `${t.id} ${t.title}`,
        detail: taskDetail(t),
        match: [t.id, t.title],
        task: t,
        dim: t.state === "merged" || t.state === "cancelled",
        struck: false,
        stalled: false,
        needs: 0,
        children: [],
      });
    }
    // Agents whose parent is gone and that no task holds.
    for (const a of mine.filter((a) => !placed.has(a.id)).sort(byOrder)) kids.push(agentNode(a, undefined, byOrder));
    const merged = tasks.filter((t) => t.state === "merged").length;
    const status = planStatus(p.state, p.paused);
    nodes.push({
      key: p.id,
      kind: "plan",
      status,
      label: p.title,
      detail: tasks.length ? `${merged}/${tasks.length}` : "",
      badge: status.label,
      match: [p.id, p.title],
      plan: p,
      dim: p.state === "done" || p.state === "cancelled",
      struck: false,
      stalled: false,
      needs: mine.reduce((n, a) => n + (needs.get(a.id) ?? 0), 0),
      children: kids,
    });
  }

  const free: TreeNode[] = [];
  const noPlan = v.agents.filter((a) => !placed.has(a.id) && !v.plans.some((p) => a.id.startsWith(`${p.id}/`)));
  for (const a of noPlan.filter((a) => !a.parent || !noPlan.some((x) => x.id === a.parent)))
    free.push(agentNode(a, undefined, () => 0));
  for (const id of v.loose) {
    const t = v.terminals.find((x) => x.id === id);
    if (!t) continue;
    const kind = typeof t.kind === "object" ? t.kind.command : t.kind;
    const label = t.title ?? kind;
    free.push({
      key: `term-${id}`,
      kind: "term",
      status: TERMINAL_STATUS,
      label,
      detail: "",
      match: [label, t.cwd],
      term: id,
      cwd: t.cwd,
      dim: false,
      struck: false,
      stalled: false,
      needs: 0,
      children: [],
    });
  }
  if (free.length)
    nodes.push({
      key: SESSIONS,
      kind: "group",
      status: AGENT_STATUS.idle,
      label: "Sessions",
      detail: "",
      match: [],
      dim: false,
      struck: false,
      stalled: false,
      needs: free.reduce((n, c) => n + c.needs, 0),
      children: free,
    });
  return nodes;
}

/**
 * The nodes that match `query` (case-insensitive, on path, task id and
 * title), with their parents kept so a match stays in place. A node that
 * matches itself keeps its whole subtree.
 */
export function filterTree(nodes: TreeNode[], query: string): TreeNode[] {
  const q = query.trim().toLowerCase();
  if (!q) return nodes;
  const walk = (list: TreeNode[]): TreeNode[] =>
    list.flatMap((n) => {
      if (n.match.some((m) => m.toLowerCase().includes(q))) return [n];
      const kids = walk(n.children);
      return kids.length ? [{ ...n, children: kids }] : [];
    });
  return walk(nodes);
}

export interface VisibleRow {
  node: TreeNode;
  /** 1 for the top level, as `aria-level` counts. */
  level: number;
  parent: string | null;
}

/** The rows on screen in order: children only under expanded nodes. */
export function visibleRows(nodes: TreeNode[], expanded: (n: TreeNode) => boolean): VisibleRow[] {
  const out: VisibleRow[] = [];
  const walk = (list: TreeNode[], level: number, parent: string | null) => {
    for (const n of list) {
      out.push({ node: n, level, parent });
      if (n.children.length && expanded(n)) walk(n.children, level + 1, n.key);
    }
  };
  walk(nodes, 1, null);
  return out;
}

/** The agent ids in tree order, every level expanded. */
export function agentOrder(v: StateView): string[] {
  return visibleRows(buildTree(v), () => true).flatMap((r) => (r.node.agent ? [r.node.agent.id] : []));
}

/** Whether a node starts expanded: finished plans start collapsed. */
export function expandedByDefault(n: TreeNode): boolean {
  return !(n.kind === "plan" && n.dim);
}

export type RowActionId = "chat" | "terminal" | "board" | "stop" | "resume" | "copy_branch" | "copy_path" | "open_pr";

export interface RowAction {
  id: RowActionId;
  label: string;
}

/** The row's menu. Mutations (stop, resume) are left out on a read-only device. */
export function rowActions(n: TreeNode, readOnly: boolean): RowAction[] {
  const out: RowAction[] = [];
  const a = n.agent;
  if (n.kind === "plan") out.push({ id: "board", label: "Open board" });
  if (a) out.push({ id: "chat", label: "Open chat" });
  if (a?.term != null || n.term !== undefined) out.push({ id: "terminal", label: "Open terminal" });
  if (n.kind === "task" && !a && n.task) out.push({ id: "board", label: "Open board" });
  if (a && !readOnly) {
    const ended = typeof a.life === "object" || a.life === "stopped";
    if (!ended) out.push({ id: "stop", label: "Stop" });
    if (ended) out.push({ id: "resume", label: "Resume" });
  }
  if (n.task?.branch) out.push({ id: "copy_branch", label: "Copy branch" });
  if (rowPath(n)) out.push({ id: "copy_path", label: "Copy path" });
  if (n.task?.pr) out.push({ id: "open_pr", label: "Open PR" });
  return out;
}

/** The directory a row works in. */
export function rowPath(n: TreeNode): string | null {
  return n.task?.worktree ?? n.agent?.cwd ?? n.cwd ?? null;
}
