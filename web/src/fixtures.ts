// Test fixtures: small, valid wire objects.

import type { Api } from "./api/http";
import type { AgentView, ChatEntry, ItemView, PlanView, StateView, TaskView, TerminalView } from "./api/types";

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

export function entry(uuid: string, over: Partial<ChatEntry> = {}): ChatEntry {
  return { uuid, ts: 1000, role: "assistant", kind: "text", text: `text ${uuid}`, ...over };
}

/** Every `Api` call, in order, as `[method, ...args]`. */
export type Call = [string, ...unknown[]];

/** An `Api` that records calls and answers with empty successes. */
export function fakeApi(over: Partial<Api> = {}): Api & { calls: Call[] } {
  const calls: Call[] = [];
  const rec =
    <T,>(name: string, value: T) =>
    async (...args: unknown[]) => {
      calls.push([name, ...args]);
      return value;
    };
  const base: Api = {
    answer: rec("answer", undefined),
    message: rec("message", undefined),
    stopAgent: rec("stopAgent", undefined),
    resumeAgent: rec("resumeAgent", undefined),
    plan: rec("plan", undefined),
    createPlan: rec("createPlan", { id: "P-20261018-new" }),
    transcript: rec("transcript", { agent: "", entries: [] }),
    repos: rec("repos", { repos: [] }),
    addRepo: rec("addRepo", { path: "/srv/repos/x", base_branch: "main", remote_url: null }),
    devices: rec("devices", { devices: [] }),
    revokeDevice: rec("revokeDevice", undefined),
    status: rec("status", {
      server: { version: "0", name: "t" },
      uptime_secs: 0,
      rev: 0,
      load: null,
      live_processes: 0,
      clients: 0,
      agents: [],
      github: "",
    }),
    settings: rec("settings", {
      tasks_per_plan: 2,
      subagents_per_task: 2,
      live_processes: 8,
      permission_wait_secs: 600,
      claude: [],
    }),
  } as Api;
  return Object.assign({ ...base, ...over }, { calls });
}
