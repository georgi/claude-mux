// Test fixtures: small, valid wire objects.

import type { AgentView, ItemView, PlanView, StateView, TaskView, TerminalView } from "./api/types";

export const PLAN = "P-20261018-demo";

export function plan(over: Partial<PlanView> = {}): PlanView {
  return {
    id: PLAN,
    title: "demo",
    state: "accepted",
    repos: [{ path: "/r", base_branch: "main", remote_url: null }],
    paused: false,
    limits: { tasks_per_plan: 2, subagents_per_task: 2 },
    tasks: [],
    agents: [],
    queue: [],
    ...over,
  };
}

export function task(id: string, over: Partial<TaskView> = {}): TaskView {
  return {
    plan: PLAN,
    id,
    title: `task ${id}`,
    state: "todo",
    repo: 0,
    deps: [],
    criteria_done: 0,
    criteria_total: 2,
    notes: 0,
    branch: null,
    worktree: null,
    pr: null,
    agent: null,
    attempt: 0,
    ...over,
  };
}

export function agent(path: string, over: Partial<AgentView> = {}): AgentView {
  const level = path === "executor" ? "executor" : path.includes("/") ? "sub" : "task";
  const parent =
    level === "executor" ? null : level === "task" ? `${PLAN}/executor` : `${PLAN}/${path.split("/")[0]}`;
  return {
    id: `${PLAN}/${path}`,
    path,
    level,
    parent,
    task: level === "executor" ? null : path.split("/")[0],
    term: null,
    cwd: "/wt",
    life: "running",
    activity: "working",
    report: null,
    tokens: { input: 0, output: 0, cache_read: 0, cache_write: 0 },
    idle_since: null,
    stalled: false,
    ...over,
  };
}

export function terminal(id: number, over: Partial<TerminalView> = {}): TerminalView {
  return {
    id,
    kind: "claude",
    agent: null,
    cwd: "/wt",
    rows: 24,
    cols: 80,
    size_owner: null,
    title: null,
    ...over,
  };
}

export function item(id: number, over: Partial<ItemView> = {}): ItemView {
  return {
    id,
    to: "human",
    kind: "question",
    from: `${PLAN}/T-0001`,
    body: "which db?",
    status: "pending",
    created: 1000,
    needs_human: true,
    ...over,
  };
}

export function state(over: Partial<StateView> = {}): StateView {
  return { rev: 1, plans: [], tasks: [], agents: [], terminals: [], loose: [], items: [], ...over };
}
