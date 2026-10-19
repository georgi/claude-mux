// One mapping from every state the daemon reports to how the UI shows it:
// a status color token (src/index.css `--status-*`), a label for text and
// screen readers, and a lucide icon name. Views render these; they do not
// pick colors or icons themselves.

import type { AgentView, CiMark, PlanState, TaskState } from "../api/types";

/** The `--status-*` tokens; Tailwind exposes them as `bg-status-<token>` etc. */
export type StatusToken =
  | "working"
  | "idle"
  | "needs"
  | "stalled"
  | "failing"
  | "passing"
  | "merged"
  | "blocked"
  | "cancelled";

/** The lucide icons the statuses use (component name without the `Icon` suffix). */
export type StatusIconName =
  | "Ban"
  | "Check"
  | "Circle"
  | "CircleCheck"
  | "CircleDashed"
  | "CircleDot"
  | "CircleHelp"
  | "CircleSlash"
  | "CircleX"
  | "Diamond"
  | "FilePen"
  | "FlaskConical"
  | "Flag"
  | "GitMerge"
  | "LoaderCircle"
  | "Pause"
  | "Square"
  | "SquareTerminal"
  | "TriangleAlert"
  | "X";

export interface Status {
  token: StatusToken;
  label: string;
  icon: StatusIconName;
}

const s = (token: StatusToken, label: string, icon: StatusIconName): Status => ({ token, label, icon });

export const TASK_STATUS: Record<TaskState, Status> = {
  todo: s("idle", "To do", "Circle"),
  in_progress: s("working", "In progress", "CircleDot"),
  testing: s("working", "Testing", "FlaskConical"),
  failing: s("failing", "Failing", "CircleX"),
  passing: s("passing", "Passing", "CircleCheck"),
  merged: s("merged", "Merged", "GitMerge"),
  blocked: s("blocked", "Blocked", "Ban"),
  cancelled: s("cancelled", "Cancelled", "CircleSlash"),
};

export const PLAN_STATUS: Record<PlanState, Status> = {
  draft: s("idle", "Draft", "FilePen"),
  proposed: s("needs", "Proposed", "CircleDot"),
  accepted: s("working", "Accepted", "Diamond"),
  done: s("passing", "Done", "Check"),
  cancelled: s("cancelled", "Cancelled", "CircleSlash"),
};

export const PLAN_PAUSED: Status = s("idle", "Paused", "Pause");

export const CI_STATUS: Record<CiMark, Status> = {
  pending: s("working", "CI pending", "CircleDashed"),
  passing: s("passing", "CI passing", "CircleCheck"),
  failing: s("failing", "CI failing", "CircleX"),
};

/** Agent statuses, keyed by what decides them (life first, then stalled, then activity). */
export const AGENT_STATUS = {
  done: s("passing", "Done", "Check"),
  failed: s("failing", "Failed", "X"),
  stopped: s("cancelled", "Stopped", "Square"),
  starting: s("idle", "Starting", "Circle"),
  stalled: s("stalled", "Stalled", "TriangleAlert"),
  working: s("working", "Working", "LoaderCircle"),
  needs_permission: s("needs", "Needs permission", "Flag"),
  idle: s("idle", "Idle", "Diamond"),
  unknown: s("idle", "Unknown", "CircleHelp"),
} satisfies Record<string, Status>;

/** A loose terminal in the tree. */
export const TERMINAL_STATUS: Status = s("idle", "Terminal", "SquareTerminal");

export function taskStatus(state: TaskState): Status {
  return TASK_STATUS[state];
}

export function planStatus(state: PlanState, paused = false): Status {
  return paused ? PLAN_PAUSED : PLAN_STATUS[state];
}

export function ciStatus(ci: CiMark): Status {
  return CI_STATUS[ci];
}

/** The same precedence as `agentGlyph` in src/tree.ts. */
export function agentStatus(a: Pick<AgentView, "life" | "activity" | "stalled" | "report">): Status {
  if (typeof a.life === "object")
    return a.life.exited.code === 0 || a.report?.status === "success" ? AGENT_STATUS.done : AGENT_STATUS.failed;
  if (a.life === "stopped") return AGENT_STATUS.stopped;
  if (a.life === "starting") return AGENT_STATUS.starting;
  if (a.stalled) return AGENT_STATUS.stalled;
  return AGENT_STATUS[a.activity];
}
