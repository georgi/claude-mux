import { describe, expect, it } from "vitest";
import type { Activity, AgentView, CiMark, PlanState, TaskState } from "../api/types";
import { agent } from "../fixtures";
import { agentGlyph } from "../tree";
import {
  AGENT_STATUS,
  agentStatus,
  CI_STATUS,
  ciStatus,
  PLAN_PAUSED,
  PLAN_STATUS,
  planStatus,
  type Status,
  TASK_STATUS,
  taskStatus,
} from "./status";

// Every value of each wire enum. `satisfies` breaks the build when the
// generated types gain a value that is missing here.
const TASK_STATES = ["todo", "in_progress", "testing", "failing", "passing", "merged", "blocked", "cancelled"] as const satisfies readonly TaskState[];
const PLAN_STATES = ["draft", "proposed", "accepted", "done", "cancelled"] as const satisfies readonly PlanState[];
const CI_MARKS = ["pending", "passing", "failing"] as const satisfies readonly CiMark[];
const ACTIVITIES = ["working", "idle", "needs_permission", "unknown"] as const satisfies readonly Activity[];
const TOKENS = ["working", "idle", "needs", "stalled", "failing", "passing", "merged", "blocked", "cancelled"];

function valid(st: Status) {
  expect(TOKENS).toContain(st.token);
  expect(st.label).toMatch(/^[A-Z]/);
  expect(st.icon).toMatch(/^[A-Z][A-Za-z]*$/);
}

describe("status mapping", () => {
  it("maps every TaskState", () => {
    expect(Object.keys(TASK_STATUS).sort()).toEqual([...TASK_STATES].sort());
    for (const s of TASK_STATES) valid(taskStatus(s));
    expect(taskStatus("merged").token).toBe("merged");
    expect(taskStatus("blocked").token).toBe("blocked");
  });

  it("maps every PlanState, and paused wins", () => {
    expect(Object.keys(PLAN_STATUS).sort()).toEqual([...PLAN_STATES].sort());
    for (const s of PLAN_STATES) {
      valid(planStatus(s));
      expect(planStatus(s, true)).toBe(PLAN_PAUSED);
    }
  });

  it("maps every CiMark", () => {
    expect(Object.keys(CI_STATUS).sort()).toEqual([...CI_MARKS].sort());
    for (const c of CI_MARKS) valid(ciStatus(c));
    expect(ciStatus("failing").token).toBe("failing");
  });

  it("maps every Activity, Life and the stalled flag like the TUI glyph", () => {
    const cases: [Partial<AgentView>, Status, string][] = [
      ...ACTIVITIES.map((activity) => [{ activity }, AGENT_STATUS[activity], ""] as [Partial<AgentView>, Status, string]),
      [{ stalled: true }, AGENT_STATUS.stalled, "!"],
      [{ life: "starting", stalled: true }, AGENT_STATUS.starting, "○"],
      [{ life: "stopped" }, AGENT_STATUS.stopped, "■"],
      [{ life: { exited: { code: 0 } } }, AGENT_STATUS.done, "✓"],
      [{ life: { exited: { code: 1 } } }, AGENT_STATUS.failed, "✗"],
      [{ life: { exited: { code: null } } }, AGENT_STATUS.failed, "✗"],
    ];
    const glyphs: Record<string, string> = { working: "*", idle: "◆", needs_permission: "⚑", unknown: "?" };
    for (const [over, want, glyph] of cases) {
      const a = agent("T-0001", over);
      valid(agentStatus(a));
      expect(agentStatus(a)).toBe(want);
      expect(agentGlyph(a, "*")).toBe(glyph || glyphs[a.activity]);
    }
    for (const st of Object.values(AGENT_STATUS)) valid(st);
  });
});
