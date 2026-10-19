import { describe, expect, it } from "vitest";
import { type DagLayout, type DagInput, layout, NODE_H, NODE_W } from "./dag";

function ok(input: DagInput[]): Extract<DagLayout, { ok: true }> {
  const r = layout(input);
  if (!r.ok) throw new Error(`unexpected cycle ${r.cycle.join(",")}`);
  return r;
}

function noOverlap(r: Extract<DagLayout, { ok: true }>) {
  for (const a of r.nodes)
    for (const b of r.nodes) {
      if (a === b) continue;
      const apart = a.x + NODE_W <= b.x || b.x + NODE_W <= a.x || a.y + NODE_H <= b.y || b.y + NODE_H <= a.y;
      expect(apart, `${a.id} overlaps ${b.id}`).toBe(true);
    }
  for (const n of r.nodes) {
    expect(n.x).toBeGreaterThanOrEqual(0);
    expect(n.y).toBeGreaterThanOrEqual(0);
    expect(n.x + NODE_W).toBeLessThanOrEqual(r.width);
    expect(n.y + NODE_H).toBeLessThanOrEqual(r.height);
  }
}

const t = (id: string, ...deps: string[]): DagInput => ({ id, deps });

describe("layout", () => {
  it("puts a chain one task per layer, left to right", () => {
    const r = ok([t("c", "b"), t("a"), t("b", "a")]);
    expect(r.layers).toEqual([["a"], ["b"], ["c"]]);
    expect(r.edges.map((e) => `${e.from}>${e.to}`).sort()).toEqual(["a>b", "b>c"]);
    const x = Object.fromEntries(r.nodes.map((n) => [n.id, n.x]));
    expect(x.a).toBeLessThan(x.b);
    expect(x.b).toBeLessThan(x.c);
    // One row: every node on the same line.
    expect(new Set(r.nodes.map((n) => n.y)).size).toBe(1);
    noOverlap(r);
  });

  it("lays out a diamond in three layers with the middle stacked", () => {
    const r = ok([t("a"), t("b", "a"), t("c", "a"), t("d", "b", "c")]);
    expect(r.layers.map((l) => [...l].sort())).toEqual([["a"], ["b", "c"], ["d"]]);
    expect(r.edges).toHaveLength(4);
    const n = Object.fromEntries(r.nodes.map((x) => [x.id, x]));
    expect(n.b.x).toBe(n.c.x);
    expect(n.b.y).not.toBe(n.c.y);
    // The single-node layers sit centered between the stacked pair.
    expect(n.a.y).toBe((n.b.y + n.c.y) / 2);
    expect(n.d.y).toBe(n.a.y);
    for (const e of r.edges) expect(e.path).toMatch(/^M[\d.]+,[\d.]+ C/);
    noOverlap(r);
  });

  it("uses the longest path: a task sits right of its deepest dep", () => {
    const r = ok([t("a"), t("b", "a"), t("c", "a", "b")]);
    expect(r.layers).toEqual([["a"], ["b"], ["c"]]);
  });

  it("stacks disconnected tasks in the first layer without overlap", () => {
    const r = ok([t("x"), t("y"), t("z"), t("w", "q")]);
    // A dep outside the plan is ignored.
    expect(r.layers).toEqual([["x", "y", "z", "w"]]);
    expect(r.edges).toEqual([]);
    noOverlap(r);
  });

  it("orders a layer by its deps to avoid crossings", () => {
    const r = ok([t("a"), t("b"), t("b2", "b"), t("a2", "a")]);
    expect(r.layers[1]).toEqual(["a2", "b2"]);
  });

  it("rejects a cycle and names the tasks on it", () => {
    const r = layout([t("a"), t("b", "a", "d"), t("c", "b"), t("d", "c"), t("e", "d")]);
    expect(r).toEqual({ ok: false, cycle: ["b", "c", "d"] });
    expect(layout([t("s", "s")])).toEqual({ ok: false, cycle: ["s"] });
  });

  it("lays out nothing for no tasks", () => {
    const r = ok([]);
    expect(r).toMatchObject({ nodes: [], edges: [], width: 0, height: 0, layers: [] });
  });
});
