import { ColumnsIcon, ListIcon, NetworkIcon, PlusIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import type { Api } from "../api/http";
import type { PlanView, RepoInfo, StateView, TaskState, TaskView } from "../api/types";
import { useSpinner } from "../hooks";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from "@/components/ui/item";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Textarea } from "@/components/ui/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { TaskGraph } from "./board/graph";
import { PlanHeader } from "./board/plan-header";
import { errText, PlanBadge, TASK_ORDER, TaskBadge } from "./board/state";
import { TaskCard, TaskRow } from "./board/task-card";
import { TaskSheet } from "./board/task-sheet";

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

      {tasks.length === 0 ? (
        <Empty>
          <EmptyHeader>
            <EmptyTitle>No tasks yet</EmptyTitle>
            <EmptyDescription>The executor adds tasks as the plan takes shape.</EmptyDescription>
          </EmptyHeader>
        </Empty>
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

/** The phone's Plans tab: every plan, newest first. */
export function PlanList({ view, onOpen, onNew }: { view: StateView; onOpen: (plan: string) => void; onNew: () => void }) {
  const plans = [...view.plans].sort((a, b) => (a.id < b.id ? 1 : -1));
  const progress = (p: PlanView) => {
    const ts = view.tasks.filter((t) => t.plan === p.id);
    const m = ts.filter((t) => t.state === "merged").length;
    return { m, n: ts.length };
  };
  return (
    <section className="mx-auto flex w-full max-w-2xl flex-col gap-3 p-4">
      {plans.length === 0 ? (
        <Empty>
          <EmptyHeader>
            <EmptyTitle>No plans yet</EmptyTitle>
            <EmptyDescription>Create a plan to start a team of agents.</EmptyDescription>
          </EmptyHeader>
        </Empty>
      ) : (
        <ItemGroup className="gap-2">
          {plans.map((p) => {
            const { m, n } = progress(p);
            return (
              <Item key={p.id} variant="outline" render={<button type="button" onClick={() => onOpen(p.id)} className="cursor-pointer text-left hover:bg-muted" />}>
                <ItemContent>
                  <ItemTitle>{p.title}</ItemTitle>
                  <ItemDescription>
                    {m}/{n} merged
                  </ItemDescription>
                  <Progress value={n ? Math.round((m / n) * 100) : 0} aria-label="Plan progress" className="w-full" />
                </ItemContent>
                <ItemActions>
                  <PlanBadge state={p.state} paused={p.paused} />
                </ItemActions>
              </Item>
            );
          })}
        </ItemGroup>
      )}
      <Button className="self-start" onClick={onNew}>
        <PlusIcon /> New plan
      </Button>
    </section>
  );
}

/**
 * The repositories the server knows, with a way to add one: a `git clone`
 * URL, or a path on the server.
 */
export function useRepos(api: Api): {
  repos: RepoInfo[] | null;
  error: string | null;
  add: (spec: string) => Promise<RepoInfo>;
} {
  const [repos, setRepos] = useState<RepoInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api
      .repos()
      .then((r) => setRepos(r.repos))
      .catch((e: unknown) => setError(errText(e)));
  }, [api]);
  const add = async (spec: string) => {
    const s = spec.trim();
    const r = await api.addRepo(s.startsWith("/") ? { path: s } : { url: s });
    setRepos((list) => [...(list ?? []).filter((x) => x.path !== r.path), r].sort((a, b) => (a.path < b.path ? -1 : 1)));
    return r;
  };
  return { repos, error, add };
}

export function AddRepo({ add, disabled }: { add: (spec: string) => Promise<RepoInfo>; disabled?: boolean }) {
  const [spec, setSpec] = useState("");
  const [busy, setBusy] = useState(false);
  // Not a <form>: NewPlan renders this inside its own form, and forms must not nest.
  const submit = async () => {
    if (!spec.trim() || busy) return;
    setBusy(true);
    try {
      const r = await add(spec);
      setSpec("");
      toast.success(`Added ${r.path}`);
    } catch (x) {
      toast.error(errText(x));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="flex gap-2">
      <Input
        aria-label="Repository URL or path"
        onKeyDown={(e) => {
          if (e.key !== "Enter") return;
          e.preventDefault();
          void submit();
        }}
        placeholder="git URL to clone, or /path on the server"
        value={spec}
        disabled={disabled}
        onChange={(e) => setSpec(e.target.value)}
      />
      <Button type="button" disabled={disabled || busy || !spec.trim()} onClick={() => void submit()}>
        {busy ? "Adding…" : "Add"}
      </Button>
    </div>
  );
}

/** New plan (UI design 4.2): a title, repositories, a first message for the executor. */
export function NewPlan({ api, readOnly, onCreated }: { api: Api; readOnly?: boolean; onCreated: (plan: string) => void }) {
  const { repos, error: repoError, add } = useRepos(api);
  const [title, setTitle] = useState("");
  const [picked, setPicked] = useState<string[]>([]);
  const [request, setRequest] = useState("");
  const [busy, setBusy] = useState(false);

  const toggle = (path: string) => setPicked((p) => (p.includes(path) ? p.filter((x) => x !== path) : [...p, path]));

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      const r = await api.createPlan({
        title: title.trim(),
        repos: picked.map((path) => ({ path, base_branch: null })),
        ...(request.trim() ? { request: request.trim() } : {}),
      });
      onCreated(r.id);
    } catch (x) {
      toast.error(errText(x));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="mx-auto w-full max-w-2xl p-4">
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">New plan</CardTitle>
          <CardDescription>Pick the repositories and tell the executor what to do.</CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="plan-title">Title</Label>
              <Input id="plan-title" value={title} onChange={(e) => setTitle(e.target.value)} required />
            </div>
            <fieldset className="flex flex-col gap-2">
              <legend className="mb-1 text-sm font-medium">Repositories</legend>
              {repoError && <p className="text-sm text-destructive">{repoError}</p>}
              {repos === null && !repoError && <p className="text-sm text-muted-foreground">Loading…</p>}
              {repos?.length === 0 && <p className="text-sm text-muted-foreground">The server knows no repositories yet. Add one below.</p>}
              {repos?.map((r) => (
                <label key={r.path} className="flex items-center gap-2 text-sm">
                  <input type="checkbox" className="size-4 accent-primary" checked={picked.includes(r.path)} onChange={() => toggle(r.path)} />
                  <code className="min-w-0 break-all">{r.path}</code>
                  {picked[0] === r.path && <Badge variant="secondary">executor runs here</Badge>}
                </label>
              ))}
            </fieldset>
            <AddRepo
              disabled={readOnly}
              add={async (s) => {
                const r = await add(s);
                setPicked((p) => (p.includes(r.path) ? p : [...p, r.path]));
                return r;
              }}
            />
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="plan-request">First message for the executor</Label>
              <Textarea id="plan-request" rows={4} value={request} placeholder="What should this plan do?" onChange={(e) => setRequest(e.target.value)} />
            </div>
            <div>
              <Button type="submit" disabled={readOnly || busy || !title.trim() || picked.length === 0}>
                {busy ? "Creating…" : "Create plan"}
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>
    </section>
  );
}
