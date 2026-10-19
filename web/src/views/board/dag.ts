// A layered layout for the task dependency graph, without a graph library:
// longest-path layering (a task sits one layer right of its deepest dep),
// then a few barycenter sweeps to cut edge crossings, then fixed-size boxes.

export interface DagInput {
  id: string;
  deps: string[];
}

export interface DagNode {
  id: string;
  layer: number;
  /** Position inside the layer, from the top. */
  order: number;
  x: number;
  y: number;
}

export interface DagEdge {
  from: string;
  to: string;
  /** An SVG path: a smooth curve from the dep's right side to the task's left side. */
  path: string;
}

export type DagLayout =
  | { ok: true; nodes: DagNode[]; edges: DagEdge[]; width: number; height: number; layers: string[][] }
  | { ok: false; cycle: string[] };

export const NODE_W = 184;
export const NODE_H = 56;
const GAP_X = 64;
const GAP_Y = 20;
const PAD = 8;
const SWEEPS = 4;

/** A cubic curve between two points that leaves and enters horizontally. */
export function curve(x1: number, y1: number, x2: number, y2: number): string {
  const dx = Math.max(24, (x2 - x1) / 2);
  return `M${x1},${y1} C${x1 + dx},${y1} ${x2 - dx},${y2} ${x2},${y2}`;
}

/**
 * Lays out `tasks` left to right. Deps on ids outside `tasks` are ignored.
 * A cycle cannot be layered: the result then names the tasks on cycles.
 */
export function layout(tasks: DagInput[]): DagLayout {
  const ids = new Set(tasks.map((t) => t.id));
  const deps = new Map(tasks.map((t) => [t.id, [...new Set(t.deps)].filter((d) => ids.has(d) && d !== t.id)]));
  const selfLoops = tasks.filter((t) => t.deps.includes(t.id)).map((t) => t.id);

  // Kahn's algorithm gives a topological order or leaves the cycles behind.
  const indeg = new Map(tasks.map((t) => [t.id, deps.get(t.id)!.length]));
  const users = new Map<string, string[]>(tasks.map((t) => [t.id, []]));
  for (const t of tasks) for (const d of deps.get(t.id)!) users.get(d)!.push(t.id);
  const queue = tasks.filter((t) => indeg.get(t.id) === 0).map((t) => t.id);
  const topo: string[] = [];
  while (queue.length) {
    const id = queue.shift()!;
    topo.push(id);
    for (const u of users.get(id)!) {
      indeg.set(u, indeg.get(u)! - 1);
      if (indeg.get(u) === 0) queue.push(u);
    }
  }
  if (topo.length < tasks.length || selfLoops.length) {
    const left = new Set(tasks.map((t) => t.id).filter((id) => !topo.includes(id)));
    // Tasks that only wait on a cycle are not on it: peel them off.
    let changed = true;
    while (changed) {
      changed = false;
      for (const id of left) {
        if (!(users.get(id) ?? []).some((u) => left.has(u))) {
          left.delete(id);
          changed = true;
        }
      }
    }
    return { ok: false, cycle: tasks.map((t) => t.id).filter((id) => left.has(id) || selfLoops.includes(id)) };
  }

  // Longest path from a root.
  const layerOf = new Map<string, number>();
  for (const id of topo) layerOf.set(id, Math.max(-1, ...deps.get(id)!.map((d) => layerOf.get(d)!)) + 1);
  const count = Math.max(0, ...layerOf.values()) + (tasks.length ? 1 : 0);
  const layers: string[][] = Array.from({ length: count }, () => []);
  // Input order seeds the order inside each layer.
  for (const t of tasks) layers[layerOf.get(t.id)!].push(t.id);

  // Barycenter sweeps: down by deps, then up by users.
  const pos = new Map<string, number>();
  const index = () => layers.forEach((l) => l.forEach((id, i) => pos.set(id, i)));
  index();
  const sortBy = (l: string[], near: (id: string) => string[]) => {
    const key = new Map(
      l.map((id) => {
        const n = near(id);
        return [id, n.length ? n.reduce((s, x) => s + pos.get(x)!, 0) / n.length : pos.get(id)!];
      }),
    );
    l.sort((a, b) => key.get(a)! - key.get(b)! || pos.get(a)! - pos.get(b)!);
    l.forEach((id, i) => pos.set(id, i));
  };
  for (let s = 0; s < SWEEPS; s++) {
    for (let i = 1; i < layers.length; i++) sortBy(layers[i], (id) => deps.get(id)!);
    for (let i = layers.length - 2; i >= 0; i--) sortBy(layers[i], (id) => users.get(id)!);
  }

  // Boxes: layers are columns, each centered on the tallest one.
  const rows = Math.max(0, ...layers.map((l) => l.length));
  const nodes: DagNode[] = [];
  const at = new Map<string, DagNode>();
  layers.forEach((l, layer) => {
    const offset = ((rows - l.length) * (NODE_H + GAP_Y)) / 2;
    l.forEach((id, order) => {
      const n = { id, layer, order, x: PAD + layer * (NODE_W + GAP_X), y: PAD + offset + order * (NODE_H + GAP_Y) };
      nodes.push(n);
      at.set(id, n);
    });
  });
  const edges: DagEdge[] = [];
  for (const t of tasks)
    for (const d of deps.get(t.id)!) {
      const a = at.get(d)!;
      const b = at.get(t.id)!;
      edges.push({ from: d, to: t.id, path: curve(a.x + NODE_W, a.y + NODE_H / 2, b.x, b.y + NODE_H / 2) });
    }
  return {
    ok: true,
    nodes,
    edges,
    layers,
    width: count ? 2 * PAD + count * NODE_W + (count - 1) * GAP_X : 0,
    height: rows ? 2 * PAD + rows * NODE_H + (rows - 1) * GAP_Y : 0,
  };
}
