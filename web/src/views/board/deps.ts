// Dependency checks for the draft task editor. Pure, so tests need no DOM.

export interface DepNode {
  id: string;
  deps: string[];
}

/**
 * Would `self` depending on `dep` close a cycle? It does when `dep` is `self`
 * or already waits on `self`, directly or through other tasks. `self` is
 * `null` for a task not created yet: nothing waits on it, so no cycle.
 */
export function wouldCycle(tasks: DepNode[], self: string | null, dep: string): boolean {
  if (self === null) return false;
  const deps = new Map(tasks.map((t) => [t.id, t.deps]));
  const seen = new Set<string>();
  const stack = [dep];
  while (stack.length) {
    const id = stack.pop()!;
    if (id === self) return true;
    if (seen.has(id)) continue;
    seen.add(id);
    stack.push(...(deps.get(id) ?? []));
  }
  return false;
}

/** The deps in `picked` that close a cycle for `self`, in order. */
export function cycleDeps(tasks: DepNode[], self: string | null, picked: string[]): string[] {
  return picked.filter((d) => wouldCycle(tasks, self, d));
}

/**
 * The tasks with every task after the tasks it waits on; ties keep the given
 * order. Tasks in a cycle (or with unknown deps only among themselves) go last.
 */
export function byDeps<T extends DepNode>(tasks: T[]): T[] {
  const ids = new Set(tasks.map((t) => t.id));
  const placed = new Set<string>();
  const out: T[] = [];
  let left = [...tasks];
  while (left.length) {
    const ready = left.filter((t) => t.deps.every((d) => placed.has(d) || !ids.has(d)));
    if (!ready.length) {
      out.push(...left);
      break;
    }
    for (const t of ready) placed.add(t.id);
    out.push(...ready);
    left = left.filter((t) => !placed.has(t.id));
  }
  return out;
}
