import { describe, expect, it } from "vitest";
import { agent, item, PLAN, plan, state, task, terminal } from "./fixtures";
import { buildTree, filterTree, rowActions, SESSIONS, type TreeNode, visibleRows } from "./tree";

/** `[key, children]` pairs, to compare shapes. */
type Shape = [string, Shape[]] | string;
const shape = (nodes: TreeNode[]): Shape[] =>
  nodes.map((n) => (n.children.length ? [n.label, shape(n.children)] : n.label));

const demo = () =>
  state({
    plans: [
      plan({
        tasks: ["T-0001", "T-0002", "T-0003"],
        agents: [`${PLAN}/executor`, `${PLAN}/T-0002`, `${PLAN}/T-0001`, `${PLAN}/T-0001/tests`],
      }),
    ],
    tasks: [
      task("T-0001", { title: "Add pagination", state: "testing", pr: { url: "u", number: 41, ci: null, ci_failures: 0 } }),
      task("T-0002", { title: "Fix login", state: "merged", branch: "claude/T-0002", worktree: "/wt/T-0002" }),
      task("T-0003", { title: "Write docs" }),
    ],
    agents: [
      agent("T-0001/tests", { activity: "idle" }),
      agent("T-0001", { term: 3, stalled: true }),
      agent("executor"),
      agent("T-0002", { life: { exited: { code: 1 } } }),
    ],
    terminals: [terminal(9, { title: "scratch" })],
    loose: [9],
    items: [item(1, { kind: "question", from: `${PLAN}/T-0001` })],
  });

describe("buildTree", () => {
  it("puts the executor first, tasks in plan order, sub-agents under tasks, sessions last", () => {
    const nodes = buildTree(demo());
    expect(shape(nodes)).toEqual([
      [
        "demo",
        [
          "executor",
          ["T-0001 Add pagination", ["tests"]],
          "T-0002 Fix login",
          "T-0003 Write docs",
        ],
      ],
      ["Sessions", ["scratch"]],
    ]);
    const [p, s] = nodes;
    expect(p.badge).toBe("Accepted");
    expect(p.detail).toBe("1/3");
    expect(p.needs).toBe(1);
    expect(s.key).toBe(SESSIONS);
    expect(s.children[0].term).toBe(9);
  });

  it("marks stalled, exited-without-report and needs-you rows", () => {
    const [p] = buildTree(demo());
    const [exec, t1, t2, t3] = p.children;
    expect(exec.stalled).toBe(false);
    expect(t1.stalled).toBe(true);
    expect(t1.status.token).toBe("stalled");
    expect(t1.needs).toBe(1);
    expect(t1.detail).toBe("testing #41");
    expect(t1.term).toBe(3);
    expect(t2.struck).toBe(true);
    expect(t2.dim).toBe(true);
    // A task without an agent shows its task state.
    expect(t3.agent).toBeUndefined();
    expect(t3.status.label).toBe("To do");
  });
});

describe("filterTree", () => {
  it("keeps the parents of a match and drops the rest", () => {
    const nodes = filterTree(buildTree(demo()), "tests");
    expect(shape(nodes)).toEqual([["demo", [["T-0001 Add pagination", ["tests"]]]]]);
  });

  it("matches task ids and titles, case-insensitively", () => {
    expect(shape(filterTree(buildTree(demo()), "t-0003"))).toEqual([["demo", ["T-0003 Write docs"]]]);
    expect(shape(filterTree(buildTree(demo()), "LOGIN"))).toEqual([["demo", ["T-0002 Fix login"]]]);
  });

  it("matches agent paths and keeps a matching node's subtree", () => {
    expect(shape(filterTree(buildTree(demo()), "T-0001"))).toEqual([["demo", [["T-0001 Add pagination", ["tests"]]]]]);
    expect(shape(filterTree(buildTree(demo()), "scratch"))).toEqual([["Sessions", ["scratch"]]]);
  });

  it("returns everything for an empty query and nothing for a miss", () => {
    const all = buildTree(demo());
    expect(filterTree(all, "  ")).toBe(all);
    expect(filterTree(all, "nope")).toEqual([]);
  });
});

describe("visibleRows", () => {
  it("lists children only under expanded nodes, with levels and parents", () => {
    const nodes = buildTree(demo());
    const rows = visibleRows(nodes, (n) => n.key !== `${PLAN}/T-0001`);
    expect(rows.map((r) => [r.level, r.node.label, r.parent])).toEqual([
      [1, "demo", null],
      [2, "executor", PLAN],
      [2, "T-0001 Add pagination", PLAN],
      [2, "T-0002 Fix login", PLAN],
      [2, "T-0003 Write docs", PLAN],
      [1, "Sessions", null],
      [2, "scratch", SESSIONS],
    ]);
  });
});

describe("rowActions", () => {
  it("offers mutations only on a full device", () => {
    const [p] = buildTree(demo());
    const t1 = p.children[1];
    expect(rowActions(t1, false).map((a) => a.id)).toEqual(["chat", "terminal", "stop", "copy_path", "open_pr"]);
    expect(rowActions(t1, true).map((a) => a.id)).toEqual(["chat", "terminal", "copy_path", "open_pr"]);
    const t2 = p.children[2];
    expect(rowActions(t2, false).map((a) => a.id)).toEqual(["chat", "resume", "copy_branch", "copy_path"]);
  });

  it.each(["starting", "running"] as const)("offers Stop instead of Resume for a stalled %s agent", (life) => {
    const node = buildTree(demo())[0].children[1];
    const actions = rowActions({ ...node, agent: { ...node.agent!, life, stalled: true } }, false).map((a) => a.id);
    expect(actions).toContain("stop");
    expect(actions).not.toContain("resume");
  });

  it("still offers Resume for a stopped agent", () => {
    const node = buildTree(demo())[0].children[1];
    const actions = rowActions({ ...node, agent: { ...node.agent!, life: "stopped" } }, false).map((a) => a.id);
    expect(actions).toContain("resume");
    expect(actions).not.toContain("stop");
  });
});
