import { CircleCheckIcon, CircleDotIcon, CircleIcon, ListTodoIcon, type LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import { Generic } from "./Generic";
import { list, obj, str, ToolCard, type ToolProps } from "./parts";

type State = "completed" | "in_progress" | "pending";

const MARK: Record<State, { icon: LucideIcon; label: string; className: string }> = {
  completed: { icon: CircleCheckIcon, label: "Done", className: "text-status-passing" },
  in_progress: { icon: CircleDotIcon, label: "In progress", className: "text-status-working" },
  pending: { icon: CircleIcon, label: "Pending", className: "text-muted-foreground" },
};

/** TodoWrite: the checklist (Queue/Plan pattern). Only the latest one opens by default. */
export function Todo(p: ToolProps) {
  const todos = list(p.row.input, "todos")
    .map(obj)
    .filter((t) => t !== undefined)
    .map((t) => {
      const s = str(t, "status");
      const state: State = s === "completed" || s === "in_progress" ? s : "pending";
      return { content: str(t, "content") ?? "", active: str(t, "activeForm"), state };
    });
  if (todos.length === 0) return <Generic {...p} />;
  const done = todos.filter((t) => t.state === "completed").length;
  const now = todos.find((t) => t.state === "in_progress");
  return (
    <ToolCard
      row={p.row}
      icon={<ListTodoIcon />}
      open={p.open}
      title={
        <>
          <span className="shrink-0 font-medium">Todos</span>
          {now && <span className="min-w-0 truncate">{now.active ?? now.content}</span>}
        </>
      }
      meta={`${done}/${todos.length} done`}
    >
      <ul aria-label="Todos" className="flex flex-col gap-1 text-sm">
        {todos.map((t, i) => {
          const m = MARK[t.state];
          return (
            <li key={i} className="flex items-start gap-2">
              <m.icon className={cn("mt-0.5 size-4 shrink-0", m.className)} aria-label={m.label} role="img" />
              <span
                className={cn(
                  t.state === "completed" && "text-muted-foreground line-through",
                  t.state === "in_progress" && "font-medium text-foreground",
                  t.state === "pending" && "text-foreground",
                )}
              >
                {t.content}
              </span>
            </li>
          );
        })}
      </ul>
    </ToolCard>
  );
}
