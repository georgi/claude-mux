import { useEffect, useMemo, useRef, useState } from "react";
import type { TaskView } from "../../api/types";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { taskStatus } from "@/lib/status";
import { cn } from "@/lib/utils";
import { layout, NODE_H, NODE_W } from "./dag";
import { STATUS_NODE } from "./tokens";

/** Graphs shrink to fit the width down to this scale; past it they pan. */
const MIN_SCALE = 0.6;

function clip(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

/** The plan's tasks as a dependency graph: deps on the left, click a task to open it. */
export function TaskGraph({ tasks, onOpen }: { tasks: TaskView[]; onOpen: (task: string) => void }) {
  const shape = useMemo(
    () => layout(tasks.map((t) => ({ id: t.id, deps: t.deps }))),
    // Only ids and deps move the layout; a state change only recolors.
    [tasks.map((t) => `${t.id}<${t.deps.join(",")}`).join(" ")],
  );
  const box = useRef<HTMLDivElement>(null);
  const [avail, setAvail] = useState(0);
  useEffect(() => {
    const el = box.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setAvail(el.clientWidth));
    ro.observe(el);
    setAvail(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  // Drag to pan when the graph overflows.
  const drag = useRef<{ x: number; y: number; left: number; top: number } | null>(null);
  const onPointerDown = (e: React.PointerEvent) => {
    const el = box.current;
    if (!el || e.button !== 0 || (e.target as Element).closest("[data-node]")) return;
    drag.current = { x: e.clientX, y: e.clientY, left: el.scrollLeft, top: el.scrollTop };
    el.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    const el = box.current;
    const d = drag.current;
    if (!el || !d) return;
    el.scrollLeft = d.left - (e.clientX - d.x);
    el.scrollTop = d.top - (e.clientY - d.y);
  };
  const onPointerUp = () => {
    drag.current = null;
  };

  if (!shape.ok)
    return (
      <Empty className="border">
        <EmptyHeader>
          <EmptyTitle>Dependency cycle</EmptyTitle>
          <EmptyDescription>
            {shape.cycle.join(" → ")} depend on each other, so the graph cannot be drawn. Use the List view.
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    );

  const byId = new Map(tasks.map((t) => [t.id, t]));
  const scale = avail ? Math.min(1, Math.max(MIN_SCALE, avail / shape.width)) : 1;
  const w = shape.width * scale;
  const h = shape.height * scale;

  return (
    <div
      ref={box}
      className="max-h-[70vh] cursor-grab overflow-auto rounded-xl border bg-card active:cursor-grabbing"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
    >
      <svg
        role="group"
        aria-label="Task dependency graph"
        width={w}
        height={h}
        viewBox={`0 0 ${shape.width} ${shape.height}`}
        className="mx-auto block"
      >
        <defs>
          <marker id="dag-arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto">
            <path d="M0,0 L8,4 L0,8 z" className="fill-muted-foreground" />
          </marker>
        </defs>
        <g fill="none" className="stroke-muted-foreground/60" strokeWidth={1.5}>
          {shape.edges.map((e) => (
            <path key={`${e.from}>${e.to}`} d={e.path} markerEnd="url(#dag-arrow)" data-edge={`${e.from}>${e.to}`} />
          ))}
        </g>
        {shape.nodes.map((n) => {
          const t = byId.get(n.id)!;
          const st = taskStatus(t.state);
          const open = () => onOpen(t.id);
          return (
            <g
              key={n.id}
              data-node={n.id}
              role="button"
              tabIndex={0}
              aria-label={`${t.id} ${t.title}, ${st.label}`}
              transform={`translate(${n.x},${n.y})`}
              className="group cursor-pointer outline-none focus-visible:[&>rect]:stroke-ring focus-visible:[&>rect]:stroke-[3]"
              onClick={open}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  open();
                }
              }}
            >
              <rect
                width={NODE_W}
                height={NODE_H}
                rx={10}
                strokeWidth={1.5}
                className={cn(STATUS_NODE[st.token], "group-hover:stroke-[2.5]")}
              />
              <text x={12} y={21} className="fill-muted-foreground font-mono text-[11px]">
                {t.id} · {st.label}
              </text>
              <text x={12} y={40} className="fill-foreground text-[13px] font-medium">
                {clip(t.title, 24)}
              </text>
              <title>{`${t.id} ${t.title}`}</title>
            </g>
          );
        })}
      </svg>
    </div>
  );
}
