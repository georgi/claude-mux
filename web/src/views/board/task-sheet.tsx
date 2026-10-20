import { BanIcon, CheckCircle2Icon, CircleIcon, CircleSlashIcon, GitBranchIcon, PlayIcon, RotateCwIcon, SquareTerminalIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import type { Api } from "../../api/http";
import type { StateView, TaskDetail, TaskState, TaskView } from "../../api/types";
import { Markdown } from "@/components/markdown";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { useIsMobile } from "@/hooks/use-mobile";
import { ciStatus } from "@/lib/status";
import { relativeTime, useNow } from "@/lib/time";
import { cn } from "@/lib/utils";
import { StatusIcon } from "../shell/status";
import { errText, LoadError, TaskBadge } from "./state";
import { agentsOf, PrLink, waitingOn } from "./task-card";

/** The states that may move to blocked (cmux-core `TaskState::allowed_next`). */
const BLOCKABLE: TaskState[] = ["in_progress", "testing", "failing", "passing"];
const FINAL: TaskState[] = ["merged", "cancelled"];

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-2" aria-label={title}>
      <h3 className="text-xs font-medium tracking-wide text-muted-foreground uppercase">{title}</h3>
      {children}
    </section>
  );
}

/** The criteria as a checklist (AI Elements "Task" pattern). */
function Criteria({ d }: { d: TaskDetail }) {
  const done = d.criteria.filter((c) => c.checked).length;
  return (
    <Section title="Criteria">
      {d.criteria.length === 0 ? (
        <p className="text-sm text-muted-foreground">No criteria.</p>
      ) : (
        <>
          <Progress value={Math.round((done / d.criteria.length) * 100)} aria-label={`${done} of ${d.criteria.length} criteria`}>
            <span className="text-xs text-muted-foreground">
              {done} of {d.criteria.length} done
            </span>
          </Progress>
          <ul className="flex flex-col gap-1.5" aria-label="Criteria list">
            {d.criteria.map((c, i) => (
              <li key={i} className="flex items-start gap-2 text-sm" data-checked={c.checked}>
                {c.checked ? (
                  <CheckCircle2Icon className="mt-0.5 size-4 shrink-0 text-status-passing" aria-label="Done" />
                ) : (
                  <CircleIcon className="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-label="Open" />
                )}
                <span className={cn("min-w-0 break-words", c.checked && "text-muted-foreground")}>{c.text}</span>
              </li>
            ))}
          </ul>
        </>
      )}
    </Section>
  );
}

/** The notes, newest last, as a timeline (AI Elements "Checkpoint" pattern). */
function Notes({ d }: { d: TaskDetail }) {
  const now = useNow();
  return (
    <Section title="Notes">
      {d.notes.length === 0 ? (
        <p className="text-sm text-muted-foreground">No notes yet.</p>
      ) : (
        <ol className="flex flex-col" aria-label="Notes timeline">
          {d.notes.map((n, i) => (
            <li key={i} className="relative flex gap-3 pb-4 last:pb-0">
              <span aria-hidden className="relative flex w-3 shrink-0 justify-center">
                <span className="mt-1.5 size-2 rounded-full bg-primary/70" />
                {i < d.notes.length - 1 && <span className="absolute top-4 bottom-[-0.25rem] w-px bg-border" />}
              </span>
              <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                <p className="flex flex-wrap items-baseline gap-2 text-xs text-muted-foreground">
                  <span className="font-mono">{n.by ? n.by.slice(n.by.lastIndexOf("/") + 1) : "you"}</span>
                  <time dateTime={new Date(n.at).toISOString()} title={new Date(n.at).toLocaleString()}>
                    {relativeTime(n.at, now)}
                  </time>
                </p>
                <p className="text-sm break-words whitespace-pre-wrap">{n.text}</p>
              </div>
            </li>
          ))}
        </ol>
      )}
    </Section>
  );
}

function Loading() {
  return (
    <div className="flex flex-col gap-3" role="status" aria-label="Loading the task">
      <Skeleton className="h-4 w-2/3" />
      <Skeleton className="h-4 w-full" />
      <Skeleton className="h-4 w-5/6" />
      <Skeleton className="mt-4 h-2 w-full" />
      <Skeleton className="h-4 w-3/4" />
      <Skeleton className="h-4 w-1/2" />
    </div>
  );
}

interface SheetProps {
  view: StateView;
  plan: string;
  task: string | null;
  api: Api;
  readOnly?: boolean;
  onClose: () => void;
  onOpenAgent: (agent: string) => void;
}

/** A task in full: body, criteria, notes, PR, and what the human can do to it. */
export function TaskSheet({ view, plan, task, api, readOnly, onClose, onOpenAgent }: SheetProps) {
  const mobile = useIsMobile();
  const t = task ? view.tasks.find((x) => x.plan === plan && x.id === task) : undefined;
  // Keep the last task on screen while the sheet animates out.
  const last = useRef(t);
  if (t) last.current = t;
  const shown = t ?? last.current;
  return (
    <Sheet open={!!t} onOpenChange={(o) => !o && onClose()}>
      <SheetContent
        side={mobile ? "bottom" : "right"}
        className="gap-0 data-[side=bottom]:max-h-[90dvh] data-[side=right]:w-full data-[side=right]:sm:max-w-lg"
      >
        {shown && <TaskBodyView key={`${shown.plan}/${shown.id}`} t={shown} view={view} api={api} readOnly={readOnly} onOpenAgent={onOpenAgent} />}
      </SheetContent>
    </Sheet>
  );
}

function TaskBodyView({ t, view, api, readOnly, onOpenAgent }: { t: TaskView } & Pick<SheetProps, "view" | "api" | "readOnly" | "onOpenAgent">) {
  const [d, setD] = useState<TaskDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [instructions, setInstructions] = useState("");
  const [force, setForce] = useState(false);
  // Fetch again whenever the summary says the detail moved.
  const stamp = `${t.state}|${t.criteria_done}/${t.criteria_total}|${t.notes}|${t.attempt}|${t.title}`;
  const [n, setN] = useState(0);
  useEffect(() => {
    let live = true;
    setError(null);
    api
      .task(t.plan, t.id)
      .then((r) => live && setD(r))
      .catch((e: unknown) => live && setError(errText(e)));
    return () => {
      live = false;
    };
  }, [api, t.plan, t.id, stamp, n]);

  const tasks = view.tasks.filter((x) => x.plan === t.plan);
  const { agent } = agentsOf(t, view);
  const running = agent && (agent.life === "running" || agent.life === "starting");
  const final = FINAL.includes(t.state);
  // cmux-core `check_spawnable`: an accepted plan, a todo or blocked task, no live agent.
  const accepted = view.plans.find((p) => p.id === t.plan)?.state === "accepted";
  const spawnable = accepted && (t.state === "todo" || t.state === "blocked") && !running;
  const can = !readOnly && !busy;

  const run = (p: Promise<unknown>, done: string) => {
    setBusy(true);
    p.then(() => toast.success(done))
      .catch((e: unknown) => toast.error(errText(e)))
      .finally(() => setBusy(false));
  };
  const move = (to: TaskState, done: string) =>
    run(
      api.patchTask(t.plan, t.id, { title: null, body: null, repo: null, deps: null, state: to }).then(setD),
      done,
    );
  const spawn = () => {
    const req = { instructions: instructions.trim() || null, force };
    run(
      api.spawnTask(t.plan, t.id, req).then(() => setInstructions("")),
      t.attempt > 0 ? `Retrying ${t.id}` : `Spawning ${t.id}`,
    );
  };
  const wait = waitingOn(t, tasks);

  return (
    <>
      <SheetHeader className="border-b pr-12">
        <SheetTitle className="flex flex-col gap-1">
          <code className="text-xs font-normal text-muted-foreground">{t.id}</code>
          <span className="break-words">{t.title}</span>
        </SheetTitle>
        <SheetDescription render={<div />} className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <TaskBadge state={t.state} />
          {t.attempt > 1 && <span className="text-xs">attempt {t.attempt}</span>}
          {t.branch && (
            <span className="inline-flex min-w-0 items-center gap-1 font-mono text-xs">
              <GitBranchIcon className="size-3 shrink-0" aria-hidden />
              <span className="truncate">{t.branch}</span>
            </span>
          )}
        </SheetDescription>
      </SheetHeader>

      <div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto p-4">
        {(t.pr || wait.length > 0 || t.deps.length > 0) && (
          <Section title="Status">
            {t.pr && (
              <p className="flex flex-wrap items-center gap-2 text-sm">
                <PrLink pr={t.pr} className="text-sm" />
                {t.pr.ci ? (
                  <span className="inline-flex items-center gap-1 text-muted-foreground">
                    <StatusIcon status={ciStatus(t.pr.ci)} /> {ciStatus(t.pr.ci).label}
                  </span>
                ) : (
                  <span className="text-muted-foreground">No CI yet</span>
                )}
                {t.pr.ci_failures > 0 && (
                  <span className="text-xs text-muted-foreground">
                    {t.pr.ci_failures} CI failure{t.pr.ci_failures > 1 ? "s" : ""}
                  </span>
                )}
              </p>
            )}
            {t.deps.length > 0 && (
              <p className="text-sm text-muted-foreground">
                Depends on {t.deps.join(", ")}
                {wait.length > 0 && ` · waits on ${wait.join(", ")}`}
              </p>
            )}
          </Section>
        )}

        {error && !d ? (
          <LoadError text={error} onRetry={() => setN((x) => x + 1)} />
        ) : !d ? (
          <Loading />
        ) : (
          <>
            {d.body.trim() && (
              <Section title="Description">
                <div className="text-sm">
                  <Markdown>{d.body}</Markdown>
                </div>
              </Section>
            )}
            <Criteria d={d} />
            <Notes d={d} />
            {d.instructions && (
              <Section title="Instructions">
                <p className="rounded-md bg-muted p-3 text-sm break-words whitespace-pre-wrap">{d.instructions}</p>
              </Section>
            )}
          </>
        )}

        {spawnable && (
          <Section title={t.attempt > 0 ? "Retry" : "Spawn"}>
            <Label htmlFor="spawn-instructions" className="sr-only">
              Instructions for the agent
            </Label>
            <Textarea
              id="spawn-instructions"
              rows={3}
              placeholder="Optional instructions for the agent"
              value={instructions}
              disabled={readOnly}
              onChange={(e) => setInstructions(e.target.value)}
            />
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div className="flex items-center gap-2">
                <Switch id="spawn-force" checked={force} onCheckedChange={setForce} disabled={readOnly} />
                <Label htmlFor="spawn-force" className="text-sm font-normal">
                  Force: start before the deps merge
                </Label>
              </div>
              <Button disabled={!can} onClick={spawn}>
                {t.attempt > 0 ? <RotateCwIcon /> : <PlayIcon />}
                {t.attempt > 0 ? "Retry" : "Spawn"}
              </Button>
            </div>
          </Section>
        )}
      </div>

      <div className="flex flex-wrap gap-2 border-t p-4">
        {agent && (
          <Button variant="secondary" onClick={() => onOpenAgent(agent.id)}>
            <SquareTerminalIcon /> Open agent
          </Button>
        )}
        {BLOCKABLE.includes(t.state) && (
          <Button variant="outline" disabled={!can} onClick={() => move("blocked", `${t.id} blocked`)}>
            <BanIcon /> Mark blocked
          </Button>
        )}
        {!final && (
          <AlertDialog>
            <AlertDialogTrigger render={<Button variant="outline" disabled={!can} />}>
              <CircleSlashIcon /> Cancel task
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Cancel {t.id}?</AlertDialogTitle>
                <AlertDialogDescription>
                  This stops the task agent and its sub-agents. A cancelled task cannot be resumed.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Keep task</AlertDialogCancel>
                <AlertDialogCancel variant="destructive" onClick={() => move("cancelled", `${t.id} cancelled`)}>
                  Cancel task
                </AlertDialogCancel>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        )}
      </div>
    </>
  );
}
