import { PencilIcon, PlusIcon, XIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import type { Api } from "../../api/http";
import type { PlanView, TaskView } from "../../api/types";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { byDeps, cycleDeps, wouldCycle } from "./deps";
import { FieldError } from "./field-error";
import { MarkdownField } from "./markdown-field";
import { errText } from "./state";

/** What the form edits; `id` is `null` for a new task. */
export interface Draft {
  id: string | null;
  title: string;
  body: string;
  repo: number;
  deps: string[];
  criteria: string[];
}

interface Errors {
  title?: string;
  deps?: string;
  criteria?: string;
}

/** Field errors for `d`; empty when it can be saved. */
export function validate(d: Draft, tasks: { id: string; deps: string[] }[]): Errors {
  const e: Errors = {};
  if (!d.title.trim()) e.title = "Give the task a title.";
  // The server cannot change criteria after creation, so only new tasks need one.
  if (d.id === null && !d.criteria.some((c) => c.trim())) e.criteria = "Add at least one criterion.";
  const bad = cycleDeps(tasks, d.id, d.deps);
  if (bad.length) e.deps = `${bad.join(", ")} already ${bad.length > 1 ? "wait" : "waits"} on ${d.id}. That makes a cycle.`;
  return e;
}

const empty = (o: Errors) => !o.title && !o.deps && !o.criteria;

/** Add or edit one draft task: title, body, repository, deps and criteria. */
export function TaskForm({
  plan,
  tasks,
  initial,
  api,
  onDone,
}: {
  plan: PlanView;
  tasks: TaskView[];
  initial: Draft;
  api: Api;
  onDone: () => void;
}) {
  const [d, setD] = useState(initial);
  const [errors, setErrors] = useState<Errors>({});
  const [busy, setBusy] = useState(false);
  const isNew = d.id === null;
  const set = (patch: Partial<Draft>, clear: keyof Errors) => {
    setD((x) => ({ ...x, ...patch }));
    setErrors((x) => ({ ...x, [clear]: undefined }));
  };
  const others = tasks.filter((t) => t.id !== d.id);
  const prefix = d.id ?? "new";

  const toggleDep = (dep: string) => {
    if (d.deps.includes(dep)) return set({ deps: d.deps.filter((x) => x !== dep) }, "deps");
    if (wouldCycle(tasks, d.id, dep)) {
      setErrors((x) => ({ ...x, deps: `${dep} already waits on ${d.id}. That makes a cycle.` }));
      return;
    }
    set({ deps: [...d.deps, dep] }, "deps");
  };

  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    const next = validate(d, tasks);
    setErrors(next);
    if (!empty(next)) return;
    setBusy(true);
    try {
      if (d.id === null) {
        const r = await api.createTask(plan.id, {
          title: d.title.trim(),
          body: d.body,
          repo: d.repo,
          deps: d.deps,
          criteria: d.criteria.map((c) => c.trim()).filter(Boolean),
        });
        toast.success(`Added ${r.id}`);
      } else {
        await api.patchTask(plan.id, d.id, { title: d.title.trim(), body: d.body, repo: d.repo, deps: d.deps, state: null });
        toast.success(`Saved ${d.id}`);
      }
      onDone();
    } catch (x) {
      toast.error(errText(x));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={save} noValidate aria-label={isNew ? "New task" : `Edit ${d.id}`} className="flex flex-col gap-4 rounded-xl border p-4">
      <div className="flex flex-col gap-1.5">
        <Label htmlFor={`${prefix}-title`}>Task title</Label>
        <Input
          id={`${prefix}-title`}
          value={d.title}
          aria-invalid={!!errors.title || undefined}
          aria-describedby={errors.title ? `${prefix}-title-error` : undefined}
          onChange={(e) => set({ title: e.target.value }, "title")}
        />
        <FieldError id={`${prefix}-title-error`}>{errors.title}</FieldError>
      </div>

      <MarkdownField id={`${prefix}-body`} label="Task body" value={d.body} rows={5} onChange={(body) => setD((x) => ({ ...x, body }))} />

      {plan.repos.length > 1 && (
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={`${prefix}-repo`}>Repository</Label>
          <select
            id={`${prefix}-repo`}
            className="h-8 rounded-lg border border-input bg-transparent px-2 text-sm"
            value={d.repo}
            onChange={(e) => setD((x) => ({ ...x, repo: Number(e.target.value) }))}
          >
            {plan.repos.map((r, i) => (
              <option key={r.path} value={i}>
                {r.path}
              </option>
            ))}
          </select>
        </div>
      )}

      <fieldset className="flex flex-col gap-1.5" aria-describedby={errors.deps ? `${prefix}-deps-error` : undefined}>
        <legend className="mb-1 text-sm font-medium">Depends on</legend>
        {others.length === 0 ? (
          <p className="text-sm text-muted-foreground">No other tasks yet.</p>
        ) : (
          <div className="flex max-h-48 flex-col gap-1 overflow-y-auto rounded-md border p-2">
            {others.map((t) => {
              const on = d.deps.includes(t.id);
              const cycle = !on && wouldCycle(tasks, d.id, t.id);
              return (
                <label key={t.id} className={`flex items-center gap-2 text-sm ${cycle ? "text-muted-foreground" : ""}`}>
                  <input
                    type="checkbox"
                    className="size-4 accent-primary"
                    checked={on}
                    aria-disabled={cycle || undefined}
                    onChange={() => toggleDep(t.id)}
                  />
                  <span className="font-mono text-xs">{t.id}</span>
                  <span className="min-w-0 truncate">{t.title}</span>
                  {cycle && <span className="ml-auto shrink-0 text-xs">would make a cycle</span>}
                </label>
              );
            })}
          </div>
        )}
        <FieldError id={`${prefix}-deps-error`}>{errors.deps}</FieldError>
      </fieldset>

      <fieldset className="flex flex-col gap-1.5" aria-describedby={errors.criteria ? `${prefix}-criteria-error` : undefined}>
        <legend className="mb-1 text-sm font-medium">Acceptance criteria</legend>
        {isNew ? (
          <>
            <ul aria-label="Criteria" className="flex flex-col gap-1.5">
              {d.criteria.map((c, i) => (
                <li key={i} className="flex gap-2">
                  <Input
                    aria-label={`Criterion ${i + 1}`}
                    value={c}
                    aria-invalid={(!!errors.criteria && !c.trim()) || undefined}
                    onChange={(e) => set({ criteria: d.criteria.map((x, j) => (j === i ? e.target.value : x)) }, "criteria")}
                  />
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label={`Remove criterion ${i + 1}`}
                    onClick={() => setD((x) => ({ ...x, criteria: x.criteria.filter((_, j) => j !== i) }))}
                  >
                    <XIcon />
                  </Button>
                </li>
              ))}
            </ul>
            <Button type="button" variant="outline" size="sm" className="self-start" onClick={() => set({ criteria: [...d.criteria, ""] }, "criteria")}>
              <PlusIcon /> Add criterion
            </Button>
          </>
        ) : (
          <>
            <ul aria-label="Criteria" className="list-disc pl-5 text-sm">
              {d.criteria.map((c, i) => (
                <li key={i}>{c}</li>
              ))}
            </ul>
            <p className="text-xs text-muted-foreground">Criteria are fixed once the task exists.</p>
          </>
        )}
        <FieldError id={`${prefix}-criteria-error`}>{errors.criteria}</FieldError>
      </fieldset>

      <div className="flex gap-2">
        <Button type="submit" disabled={busy}>
          {busy ? "Saving…" : isNew ? "Add task" : "Save task"}
        </Button>
        <Button type="button" variant="ghost" onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

/** The plan file, editable while the plan is a draft. */
function PlanBodyEditor({ api, plan, onDone }: { api: Api; plan: string; onDone: () => void }) {
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [load, setLoad] = useState(0);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let live = true;
    setText(null);
    setError(null);
    api
      .planBody(plan)
      .then((b) => live && setText(b.text))
      .catch((e: unknown) => {
        if (live) setError(errText(e));
      });
    return () => {
      live = false;
    };
  }, [api, plan, load]);
  if (error)
    return (
      <div className="flex flex-col gap-2 rounded-xl border p-4">
        <p role="alert" className="text-sm text-destructive">Could not load the plan body: {error}</p>
        <div className="flex gap-2">
          <Button type="button" onClick={() => setLoad((n) => n + 1)}>Retry</Button>
          <Button type="button" variant="ghost" onClick={onDone}>Cancel</Button>
        </div>
      </div>
    );
  if (text === null) return <Skeleton className="h-32 w-full" />;
  const save = () => {
    setBusy(true);
    api
      .putPlanBody(plan, { text })
      .then(() => {
        toast.success("Saved the plan body");
        onDone();
      })
      .catch((e: unknown) => toast.error(errText(e)))
      .finally(() => setBusy(false));
  };
  return (
    <div className="flex flex-col gap-2 rounded-xl border p-4">
      <MarkdownField id="draft-plan-body" label="Plan body" value={text} rows={8} onChange={setText} />
      <div className="flex gap-2">
        <Button type="button" disabled={busy} onClick={save}>
          {busy ? "Saving…" : "Save plan body"}
        </Button>
        <Button type="button" variant="ghost" onClick={onDone}>
          Cancel
        </Button>
      </div>
    </div>
  );
}

type Editing = { kind: "none" } | { kind: "body" } | { kind: "task"; draft: Draft } | { kind: "loading"; id: string };

const blank = (): Draft => ({ id: null, title: "", body: "", repo: 0, deps: [], criteria: [""] });

/** The draft plan's tasks in dependency order, with add and edit (UI design 4.2). */
export default function TaskEditor({ plan, tasks, api, readOnly }: { plan: PlanView; tasks: TaskView[]; api: Api; readOnly?: boolean }) {
  const [editing, setEditing] = useState<Editing>({ kind: "none" });
  const done = () => setEditing({ kind: "none" });
  const edit = (id: string) => {
    setEditing({ kind: "loading", id });
    api
      .task(plan.id, id)
      .then((r) =>
        setEditing({
          kind: "task",
          draft: { id, title: r.task.title, body: r.body, repo: r.task.repo, deps: r.task.deps, criteria: r.criteria.map((c) => c.text) },
        }),
      )
      .catch((e: unknown) => {
        toast.error(errText(e));
        done();
      });
  };
  const ordered = byDeps(tasks);
  const editingId = editing.kind === "task" ? editing.draft.id : editing.kind === "loading" ? editing.id : null;

  return (
    <Card size="sm" role="region" aria-labelledby="draft-tasks-title">
      <CardHeader>
        <CardTitle id="draft-tasks-title">Draft tasks</CardTitle>
        <CardDescription>Shape the tasks before the plan starts. They run in this order.</CardDescription>
        {!readOnly && (
          <CardAction className="flex gap-2">
            <Button variant="outline" size="sm" disabled={editing.kind !== "none"} onClick={() => setEditing({ kind: "body" })}>
              <PencilIcon /> Plan body
            </Button>
            <Button size="sm" disabled={editing.kind !== "none"} onClick={() => setEditing({ kind: "task", draft: blank() })}>
              <PlusIcon /> Add task
            </Button>
          </CardAction>
        )}
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {editing.kind === "body" && <PlanBodyEditor api={api} plan={plan.id} onDone={done} />}
        {editing.kind === "task" && editing.draft.id === null && (
          <TaskForm plan={plan} tasks={tasks} initial={editing.draft} api={api} onDone={done} />
        )}
        {ordered.length > 0 && (
          <ol aria-label="Draft task order" className="flex flex-col gap-2">
            {ordered.map((t) => (
              <li key={t.id} data-draft-task={t.id} className="flex flex-col gap-2">
                {editing.kind === "task" && editingId === t.id ? (
                  <TaskForm plan={plan} tasks={tasks} initial={editing.draft} api={api} onDone={done} />
                ) : (
                  <div className="flex items-center gap-3 rounded-lg border px-3 py-2 text-sm">
                    <span className="font-mono text-xs text-muted-foreground">{t.id}</span>
                    <span className="min-w-0 flex-1 truncate font-medium">{t.title}</span>
                    {t.deps.length > 0 && <span className="shrink-0 text-xs text-muted-foreground">waits on {t.deps.join(", ")}</span>}
                    <span className="shrink-0 text-xs text-muted-foreground tabular-nums">{t.criteria_total} criteria</span>
                    {!readOnly && (
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={`Edit ${t.id}`}
                        disabled={editing.kind !== "none"}
                        onClick={() => edit(t.id)}
                      >
                        <PencilIcon />
                      </Button>
                    )}
                  </div>
                )}
              </li>
            ))}
          </ol>
        )}
        {ordered.length === 0 && editing.kind === "none" && <p className="text-sm text-muted-foreground">No tasks yet. Add the first one.</p>}
      </CardContent>
    </Card>
  );
}
