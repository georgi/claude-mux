import { GitBranchIcon, GitPullRequestIcon, HourglassIcon, RotateCwIcon } from "lucide-react";
import type { AgentView, PrRef, StateView, TaskView } from "../../api/types";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Item, ItemActions, ItemContent, ItemDescription, ItemTitle } from "@/components/ui/item";
import { Progress } from "@/components/ui/progress";
import { agentStatus, ciStatus, taskStatus } from "@/lib/status";
import { cn } from "@/lib/utils";
import { StatusIcon } from "../shell/status";
import { TaskBadge } from "./state";
import { STATUS_BG } from "./tokens";

/** The deps of `t` that are not merged yet. */
export function waitingOn(t: TaskView, tasks: TaskView[]): string[] {
  return t.deps.filter((d) => tasks.find((x) => x.id === d)?.state !== "merged");
}

/** The task's agent and that agent's sub-agents. */
export function agentsOf(t: TaskView, view: StateView): { agent?: AgentView; subs: AgentView[] } {
  const agent = t.agent ? view.agents.find((x) => x.id === t.agent) : undefined;
  return { agent, subs: agent ? view.agents.filter((x) => x.parent === agent.id) : [] };
}

export function PrLink({ pr, className }: { pr: PrRef; className?: string }) {
  return (
    <a
      className={cn("relative z-10 inline-flex items-center gap-1 text-xs underline-offset-4 hover:underline", className)}
      href={pr.url}
      target="_blank"
      rel="noreferrer"
    >
      <GitPullRequestIcon className="size-3" aria-hidden />#{pr.number}
      {pr.ci && <StatusIcon status={ciStatus(pr.ci)} />}
      <span className="sr-only">(opens GitHub)</span>
    </a>
  );
}

function SubDots({ subs }: { subs: AgentView[] }) {
  if (!subs.length) return null;
  return (
    <span className="inline-flex items-center gap-1" aria-label={`${subs.length} sub-agent${subs.length > 1 ? "s" : ""}`} role="img">
      {subs.map((s) => {
        const st = agentStatus(s);
        return (
          <span
            key={s.id}
            title={`${s.path.slice(s.path.indexOf("/") + 1)}: ${st.label}`}
            className={cn("size-2 rounded-full", STATUS_BG[st.token], st.token === "working" && "animate-pulse")}
          />
        );
      })}
    </span>
  );
}

function criteriaPct(t: TaskView): number {
  return t.criteria_total ? Math.round((t.criteria_done / t.criteria_total) * 100) : 0;
}

interface TaskProps {
  t: TaskView;
  tasks: TaskView[];
  view: StateView;
  onOpen: (task: string) => void;
}

/** One task on the Columns view. The whole card opens the task. */
export function TaskCard({ t, tasks, view, onOpen }: TaskProps) {
  const { agent, subs } = agentsOf(t, view);
  const wait = t.state === "todo" ? waitingOn(t, tasks) : [];
  return (
    <Card size="sm" data-task={t.id} className="relative transition-colors hover:bg-muted/50">
      <CardHeader>
        <CardTitle className="flex items-start gap-2">
          <button
            type="button"
            className="min-w-0 flex-1 text-left outline-none after:absolute after:inset-0 after:rounded-[inherit] after:content-[''] focus-visible:after:ring-2 focus-visible:after:ring-ring"
            onClick={() => onOpen(t.id)}
          >
            <code className="mr-1.5 text-xs text-muted-foreground">{t.id}</code>
            <span className="break-words">{t.title}</span>
          </button>
          {agent && <StatusIcon status={agentStatus(agent)} className="mt-0.5" />}
        </CardTitle>
        <CardDescription className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <TaskBadge state={t.state} />
          {t.pr && <PrLink pr={t.pr} />}
          {t.attempt > 1 && (
            <span className="inline-flex items-center gap-1 text-xs">
              <RotateCwIcon className="size-3" aria-hidden />
              attempt {t.attempt}
            </span>
          )}
          <SubDots subs={subs} />
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-2">
        {t.branch && (
          <span className="inline-flex min-w-0 items-center gap-1 font-mono text-xs text-muted-foreground">
            <GitBranchIcon className="size-3 shrink-0" aria-hidden />
            <span className="truncate">{t.branch}</span>
          </span>
        )}
        {t.criteria_total > 0 && (
          <Progress value={criteriaPct(t)} aria-label={`${t.criteria_done} of ${t.criteria_total} criteria`}>
            <span className="text-xs text-muted-foreground">
              {t.criteria_done}/{t.criteria_total} criteria
            </span>
          </Progress>
        )}
        {wait.length > 0 && (
          <p className="inline-flex items-center gap-1 text-xs text-muted-foreground">
            <HourglassIcon className="size-3" aria-hidden />
            waits on {wait.join(", ")}
          </p>
        )}
      </CardContent>
    </Card>
  );
}

/** One dense row on the List view. */
export function TaskRow({ t, tasks, view, onOpen }: TaskProps) {
  const { subs } = agentsOf(t, view);
  const wait = t.state === "todo" ? waitingOn(t, tasks) : [];
  return (
    <Item
      role="listitem"
      size="xs"
      data-task={t.id}
      className="relative flex-nowrap px-3 py-2 hover:bg-muted/50"
    >
      <StatusIcon status={taskStatus(t.state)} />
      <ItemContent className="min-w-0">
        <ItemTitle className="w-full min-w-0">
          <button
            type="button"
            className="flex min-w-0 items-baseline gap-2 text-left outline-none after:absolute after:inset-0 after:rounded-[inherit] after:content-[''] focus-visible:after:ring-2 focus-visible:after:ring-ring"
            onClick={() => onOpen(t.id)}
          >
            <code className="shrink-0 text-xs text-muted-foreground">{t.id}</code>
            <span className="truncate">{t.title}</span>
          </button>
        </ItemTitle>
        {(t.branch || wait.length > 0) && (
          <ItemDescription className="flex min-w-0 gap-3 text-xs">
            {t.branch && <span className="truncate font-mono">{t.branch}</span>}
            {wait.length > 0 && <span className="shrink-0">waits on {wait.join(", ")}</span>}
          </ItemDescription>
        )}
      </ItemContent>
      <ItemActions className="shrink-0 gap-3 text-xs text-muted-foreground">
        <SubDots subs={subs} />
        {t.attempt > 1 && <span>#{t.attempt}</span>}
        {t.pr && <PrLink pr={t.pr} className="hidden sm:inline-flex" />}
        {t.criteria_total > 0 && (
          <span className="tabular-nums" aria-label={`${t.criteria_done} of ${t.criteria_total} criteria`}>
            {t.criteria_done}/{t.criteria_total}
          </span>
        )}
        <span className="hidden w-24 md:inline-flex">
          <TaskBadge state={t.state} />
        </span>
      </ItemActions>
    </Item>
  );
}
