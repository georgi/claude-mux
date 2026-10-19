import { CheckCircle2Icon, CircleSlashIcon, PauseIcon, PlayIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import type { Api } from "../../api/http";
import type { AgentView, PlanView, TaskView } from "../../api/types";
import { Markdown } from "@/components/markdown";
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from "@/components/ui/accordion";
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
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { Skeleton } from "@/components/ui/skeleton";
import { agentGlyph } from "../../tree";
import { errText, PlanBadge } from "./state";

/** The plan file, fetched once the accordion opens. */
function PlanBody({ api, plan }: { api: Api; plan: string }) {
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    api
      .planBody(plan)
      .then((b) => live && setText(b.text))
      .catch((e: unknown) => live && setError(errText(e)));
    return () => {
      live = false;
    };
  }, [api, plan]);
  if (error) return <p className="text-sm text-destructive">{error}</p>;
  if (text === null)
    return (
      <div className="flex flex-col gap-2" role="status" aria-label="Loading the plan">
        <Skeleton className="h-4 w-1/2" />
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-4/5" />
      </div>
    );
  if (!text.trim()) return <p className="text-sm text-muted-foreground">The executor has not written the plan yet.</p>;
  return (
    <div className="text-sm">
      <Markdown>{text}</Markdown>
    </div>
  );
}

interface HeaderProps {
  p: PlanView;
  tasks: TaskView[];
  executor?: AgentView;
  spin: string;
  api: Api;
  readOnly?: boolean;
  onOpenAgent: (agent: string) => void;
  /** The view switcher, placed beside the actions. */
  children?: React.ReactNode;
}

/** Title, state, repos, progress and the plan controls (UI design 3.6). */
export function PlanHeader({ p, tasks, executor, spin, api, readOnly, onOpenAgent, children }: HeaderProps) {
  const [busy, setBusy] = useState(false);
  const [bodyOpen, setBodyOpen] = useState(false);
  const can = !readOnly && !busy;
  const merged = tasks.filter((t) => t.state === "merged").length;
  const act = (what: "accept" | "resume" | "pause" | "cancel", done: string) => {
    setBusy(true);
    api
      .plan(p.id, what)
      .then(() => toast.success(done))
      .catch((e: unknown) => toast.error(errText(e)))
      .finally(() => setBusy(false));
  };
  const live = p.state === "draft" || p.state === "proposed" || p.state === "accepted";
  return (
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
        <div className="flex flex-wrap items-center gap-2">
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
              <span aria-hidden className="font-mono">
                ◆{agentGlyph(executor, spin)}
              </span>{" "}
              Executor
            </Button>
          )}
          {live && (
            <AlertDialog>
              <AlertDialogTrigger render={<Button variant="ghost" disabled={!can} />}>
                <CircleSlashIcon /> Cancel plan
              </AlertDialogTrigger>
              <AlertDialogContent>
                <AlertDialogHeader>
                  <AlertDialogTitle>Cancel this plan?</AlertDialogTitle>
                  <AlertDialogDescription>
                    This stops the executor and every task agent of “{p.title}”. A cancelled plan cannot be resumed.
                  </AlertDialogDescription>
                </AlertDialogHeader>
                <AlertDialogFooter>
                  <AlertDialogCancel>Keep plan</AlertDialogCancel>
                  <AlertDialogCancel variant="destructive" onClick={() => act("cancel", "Plan cancelled")}>
                    Cancel plan
                  </AlertDialogCancel>
                </AlertDialogFooter>
              </AlertDialogContent>
            </AlertDialog>
          )}
          {children && <div className="ml-auto">{children}</div>}
        </div>
        <Accordion value={bodyOpen ? ["plan"] : []} onValueChange={(v) => setBodyOpen(v.includes("plan"))}>
          <AccordionItem value="plan">
            <AccordionTrigger>Plan</AccordionTrigger>
            <AccordionContent>{bodyOpen && <PlanBody api={api} plan={p.id} />}</AccordionContent>
          </AccordionItem>
        </Accordion>
      </CardContent>
    </Card>
  );
}
