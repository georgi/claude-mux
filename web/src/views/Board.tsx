import { CheckCircle2Icon, GitBranchIcon, GitPullRequestIcon, PauseIcon, PlayIcon, PlusIcon, XCircleIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import type { Api } from "../api/http";
import type { PlanView, RepoInfo, StateView, TaskView } from "../api/types";
import { useSpinner } from "../hooks";
import { agentGlyph } from "../tree";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from "@/components/ui/item";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Textarea } from "@/components/ui/textarea";
import { errText, PlanBadge, TASK_ORDER, TaskBadge } from "./board/state";

interface BoardProps {
  view: StateView;
  plan: string;
  api: Api;
  readOnly?: boolean;
  onOpenAgent: (agent: string) => void;
}

function waits(t: TaskView, tasks: TaskView[]): string {
  const open = t.deps.filter((d) => tasks.find((x) => x.id === d)?.state !== "merged");
  return open.length ? `waits ${open.join(", ")}` : "";
}

function CiMark({ ci }: { ci: string | null | undefined }) {
  if (ci === "failing") return <XCircleIcon className="size-3.5 text-destructive" aria-label="CI failing" />;
  if (ci === "passing") return <CheckCircle2Icon className="size-3.5 text-primary" aria-label="CI passing" />;
  return null;
}

function TaskCard({ t, tasks, view, spin, onOpenAgent }: { t: TaskView; tasks: TaskView[]; view: StateView; spin: string; onOpenAgent: (a: string) => void }) {
  const a = t.agent ? view.agents.find((x) => x.id === t.agent) : undefined;
  const subs = a ? view.agents.filter((x) => x.parent === a.id) : [];
  const wait = t.state === "todo" ? waits(t, tasks) : "";
  const pct = t.criteria_total ? Math.round((t.criteria_done / t.criteria_total) * 100) : 0;
  return (
    <Card size="sm" data-task={t.id}>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          {a ? <span aria-hidden className="font-mono">{agentGlyph(a, spin)}</span> : null}
          <code className="text-xs text-muted-foreground">{t.id}</code>
          <span className="min-w-0 flex-1 truncate">{t.title}</span>
        </CardTitle>
        <CardDescription className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <TaskBadge state={t.state} />
          {t.branch && (
            <span className="inline-flex items-center gap-1 font-mono text-xs">
              <GitBranchIcon className="size-3" aria-hidden />
              {t.branch}
            </span>
          )}
          {t.pr && (
            <a
              className="inline-flex items-center gap-1 text-xs underline-offset-4 hover:underline"
              href={t.pr.url}
              target="_blank"
              rel="noreferrer"
            >
              <GitPullRequestIcon className="size-3" aria-hidden />#{t.pr.number}
              <CiMark ci={t.pr.ci} />
              <span className="sr-only">(opens GitHub)</span>
            </a>
          )}
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        {t.criteria_total > 0 && (
          <Progress value={pct} aria-label={`${t.criteria_done} of ${t.criteria_total} criteria`}>
            <span className="text-xs text-muted-foreground">
              {t.criteria_done}/{t.criteria_total} criteria
            </span>
          </Progress>
        )}
        {wait && <p className="text-xs text-muted-foreground">{wait}</p>}
        {subs.length > 0 && (
          <p className="truncate font-mono text-xs text-muted-foreground">
            {subs.map((s) => `${agentGlyph(s, spin)} ${s.path.slice(s.path.indexOf("/") + 1)}`).join("  ")}
          </p>
        )}
        {a && (
          <Button variant="outline" size="sm" className="self-start" onClick={() => onOpenAgent(a.id)}>
            Open agent
          </Button>
        )}
      </CardContent>
    </Card>
  );
}

/** One plan's tasks, grouped by state (UI design 3.6), with the plan's controls. */
export function Board({ view, plan, api, readOnly, onOpenAgent }: BoardProps) {
  const p = view.plans.find((x) => x.id === plan);
  const spin = useSpinner(view.agents.some((a) => a.activity === "working"));
  const [busy, setBusy] = useState(false);
  if (!p) return <p className="p-4 text-sm text-muted-foreground">No plan {plan}.</p>;
  const tasks = p.tasks.map((id) => view.tasks.find((t) => t.plan === p.id && t.id === id)).filter((t): t is TaskView => !!t);
  const executor = view.agents.find((a) => a.id === `${p.id}/executor`);
  const can = !readOnly && !busy;
  const merged = tasks.filter((t) => t.state === "merged").length;
  const act = (what: "accept" | "resume" | "pause", done: string) => {
    setBusy(true);
    api
      .plan(p.id, what)
      .then(() => toast.success(done))
      .catch((e: unknown) => toast.error(errText(e)))
      .finally(() => setBusy(false));
  };
  return (
    <section className="mx-auto flex w-full max-w-5xl flex-col gap-4 p-4">
      <Card>
        <CardHeader>
          <CardTitle className="flex flex-wrap items-center gap-2 text-lg">
            <span className="min-w-0 break-words">{p.title}</span>
            <PlanBadge state={p.state} paused={p.paused} />
          </CardTitle>
          <CardDescription className="break-words">
            <code>{p.id}</code> · {p.repos.map((r) => r.path).join(", ")}
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <Progress value={tasks.length ? Math.round((merged / tasks.length) * 100) : 0} aria-label="Plan progress">
            <span className="text-sm">
              {merged}/{tasks.length} merged
            </span>
          </Progress>
          <div className="flex flex-wrap gap-2">
            {(p.state === "draft" || p.state === "proposed") && (
              <Button disabled={!can} onClick={() => act("accept", "Plan accepted")}>
                <CheckCircle2Icon /> Accept plan
              </Button>
            )}
            {p.state === "accepted" &&
              (p.paused ? (
                <Button disabled={!can} onClick={() => act("resume", "Plan resumed")}>
                  <PlayIcon /> Resume
                </Button>
              ) : (
                <Button variant="outline" disabled={!can} onClick={() => act("pause", "Plan paused")}>
                  <PauseIcon /> Pause
                </Button>
              ))}
            {executor && (
              <Button variant="secondary" onClick={() => onOpenAgent(executor.id)}>
                <span aria-hidden className="font-mono">◆{agentGlyph(executor, spin)}</span> Executor
              </Button>
            )}
          </div>
        </CardContent>
      </Card>

      {tasks.length === 0 ? (
        <Empty>
          <EmptyHeader>
            <EmptyTitle>No tasks yet</EmptyTitle>
            <EmptyDescription>The executor adds tasks as the plan takes shape.</EmptyDescription>
          </EmptyHeader>
        </Empty>
      ) : (
        TASK_ORDER.map((state) => {
          const group = tasks.filter((t) => t.state === state);
          if (!group.length) return null;
          return (
            <div key={state} className="flex flex-col gap-2">
              <h3 className="flex items-center gap-2 text-sm font-medium">
                <TaskBadge state={state} />
                <span className="text-muted-foreground">{group.length}</span>
              </h3>
              <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
                {group.map((t) => (
                  <TaskCard key={t.id} t={t} tasks={tasks} view={view} spin={spin} onOpenAgent={onOpenAgent} />
                ))}
              </div>
            </div>
          );
        })
      )}
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
