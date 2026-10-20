import { describe, expect, it } from "vitest";
import { agent, PLAN, plan, state, task, terminal } from "./fixtures";
import { buildTree } from "./tree";

describe("buildTree", () => {
  it("nests agents under their parents in spawn order", () => {
    const v = state({
      plans: [
        plan({
          agents: [`${PLAN}/executor`, `${PLAN}/T-0002`, `${PLAN}/T-0001`, `${PLAN}/T-0001/tests`],
        }),
      ],
      tasks: [task("T-0001", { state: "testing", pr: { url: "u", number: 41, ci: null, ci_failures: 0 } }), task("T-0002")],
      agents: [
        agent("T-0001/tests", { activity: "idle" }),
        agent("T-0001", { term: 3 }),
        agent("executor"),
        agent("T-0002", { life: { exited: { code: 0 } } }),
      ],
      terminals: [terminal(9)],
      loose: [9],
    });
    const rows = buildTree(v, "*");
    expect(rows.map((r) => [r.depth, r.glyph, r.label, r.detail])).toEqual([
      [0, "◆", "demo", "accepted"],
      [1, "*", "executor", ""],
      [2, "✓", "T-0002 task T-0002", "todo"],
      [2, "*", "T-0001 task T-0001", "testing #41"],
      [3, "◆", "tests", ""],
      [0, "▸", "claude", ""],
    ]);
    expect(rows[3].term).toBe(3);
    expect(rows[2].dim).toBe(true);
    expect(rows[5].term).toBe(9);
  });
});
