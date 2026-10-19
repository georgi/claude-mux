import { ColumnsIcon, ListIcon, NetworkIcon } from "lucide-react";
import { lazy, Suspense, useState } from "react";
import type { Api } from "../api/http";
import type { StateView, TaskState, TaskView } from "../api/types";
import { useSpinner } from "../hooks";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { ItemGroup } from "@/components/ui/item";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { TaskGraph } from "./board/graph";
import { PlanHeader } from "./board/plan-header";
import { TASK_ORDER, TaskBadge } from "./board/state";
import { TaskCard, TaskRow } from "./board/task-card";
import { TaskSheet } from "./board/task-sheet";

// Only draft plans need the editor; load it with the first one.
const TaskEditor = lazy(() => import("./board/task-editor"));

interface BoardProps {
  view: StateView;
  plan: string;
  api: Api;
  readOnly?: boolean;
  onOpenAgent: (agent: string) => void;
}

export type BoardView = "columns" | "list" | "graph";
const VIEWS: BoardView[] = ["columns", "list", "graph"];

/** Columns in the order work flows; the first ones show even when empty. */
const COLUMNS: TaskState[] = ["todo", "in_progress", "testing", "failing", "blocked", "passing", "merged", "cancelled"];
const ALWAYS: TaskState[] = ["todo", "in_progress", "testing", "passing", "merged"];

const viewKey = (plan: string) => `cmux.board.view.${plan}`;

/** The board view last picked for `plan`, kept in this browser. */
function useBoardView(plan: string): [BoardView, (v: BoardView) => void] {
  const read = (): BoardView => {
    try {
      const v = localStorage.getItem(viewKey(plan));
      return VIEWS.includes(v as BoardView) ? (v as BoardView) : "columns";
    } catch {
      return "columns";
    }
  };
  const [state, setState] = useState(() => ({ plan, v: read() }));
  // A different plan reads its own choice.
  const v = state.plan === plan ? state.v : read();
  if (state.plan !== plan) setState({ plan, v });
  const set = (next: BoardView) => {
    setState({ plan, v: next });
    try {
      localStorage.setItem(viewKey(plan), next);
    } catch {
      // Private mode: the choice lasts for this page only.
    }
  };
  return [v, set];
}

function Columns({ tasks, view, onOpen }: { tasks: TaskView[]; view: StateView; onOpen: (t: string) => void }) {
  return (
    <div className="-mx-4 overflow-x-auto px-4 pb-2" role="region" aria-label="Task columns" tabIndex={0}>
      <div className="flex w-max gap-3">
        {COLUMNS.map((state) => {
          const group = tasks.filter((t) => t.state === state);
          if (!group.length && !ALWAYS.includes(state)) return null;
          return (
            <div key={state} className="flex w-72 shrink-0 flex-col gap-2 rounded-xl bg-muted/40 p-2" data-column={state}>
              <h3 className="flex items-center gap-2 px-1 text-sm font-medium">
                <TaskBadge state={state} />
                <span className="text-muted-foreground tabular-nums">{group.length}</span>
              </h3>
              {group.map((t) => (
                <TaskCard key={t.id} t={t} tasks={tasks} view={view} onOpen={onOpen} />
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function List({ tasks, view, onOpen }: { tasks: TaskView[]; view: StateView; onOpen: (t: string) => void }) {
  const sorted = TASK_ORDER.flatMap((s) => tasks.filter((t) => t.state === s));
  return (
    <ItemGroup className="gap-0 divide-y rounded-xl border" aria-label="Tasks">
      {sorted.map((t) => (
        <TaskRow key={t.id} t={t} tasks={tasks} view={view} onOpen={onOpen} />
      ))}
    </ItemGroup>
  );
}

/** One plan's tasks as columns, a list or a dependency graph (UI design 3.6), with the plan's controls. */
export function Board({ view, plan, api, readOnly, onOpenAgent }: BoardProps) {
  const p = view.plans.find((x) => x.id === plan);
  const spin = useSpinner(view.agents.some((a) => a.activity === "working"));
  const [mode, setMode] = useBoardView(plan);
  const [open, setOpen] = useState<string | null>(null);
  if (!p) return <p className="p-4 text-sm text-muted-foreground">No plan {plan}.</p>;
  const tasks = p.tasks.map((id) => view.tasks.find((t) => t.plan === p.id && t.id === id)).filter((t): t is TaskView => !!t);
  const executor = view.agents.find((a) => a.id === `${p.id}/executor`);
  const draft = p.state === "draft" || p.state === "proposed";
  return (
    <section className="mx-auto flex w-full max-w-7xl min-w-0 flex-col gap-4 p-4">
      <PlanHeader p={p} tasks={tasks} executor={executor} spin={spin} api={api} readOnly={readOnly} onOpenAgent={onOpenAgent}>
        {tasks.length > 0 && (
          <ToggleGroup
            variant="outline"
            size="sm"
            spacing={0}
            aria-label="Board view"
            value={[mode]}
            onValueChange={(v: unknown[]) => {
              const next = v[0] as BoardView | undefined;
              if (next) setMode(next);
            }}
          >
            <ToggleGroupItem value="columns" aria-label="Columns">
              <ColumnsIcon /> <span className="hidden sm:inline">Columns</span>
            </ToggleGroupItem>
            <ToggleGroupItem value="list" aria-label="List">
              <ListIcon /> <span className="hidden sm:inline">List</span>
            </ToggleGroupItem>
            <ToggleGroupItem value="graph" aria-label="Graph">
              <NetworkIcon /> <span className="hidden sm:inline">Graph</span>
            </ToggleGroupItem>
          </ToggleGroup>
        )}
      </PlanHeader>

      {draft && (
        <Suspense fallback={<Skeleton className="h-24 w-full" />}>
          <TaskEditor plan={p} tasks={tasks} api={api} readOnly={readOnly} />
        </Suspense>
      )}

      {tasks.length === 0 ? (
        !draft && (
          <Empty>
            <EmptyHeader>
              <EmptyTitle>No tasks yet</EmptyTitle>
              <EmptyDescription>The executor adds tasks as the plan takes shape.</EmptyDescription>
            </EmptyHeader>
          </Empty>
        )
      ) : mode === "graph" ? (
        <TaskGraph tasks={tasks} onOpen={setOpen} />
      ) : mode === "list" ? (
        <List tasks={tasks} view={view} onOpen={setOpen} />
      ) : (
        <Columns tasks={tasks} view={view} onOpen={setOpen} />
      )}

      <TaskSheet
        view={view}
        plan={p.id}
        task={open}
        api={api}
        readOnly={readOnly}
        onClose={() => setOpen(null)}
        onOpenAgent={(a) => {
          setOpen(null);
          onOpenAgent(a);
        }}
      />
    </section>
  );
}
