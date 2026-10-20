import type { PlanState, TaskState } from "../../api/types";
import { RotateCwIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { planStatus, type Status, taskStatus } from "@/lib/status";
import { STATUS_TEXT } from "../shell/status";

/** Task states in the order the board groups them. */
export const TASK_ORDER: TaskState[] = ["in_progress", "testing", "failing", "blocked", "passing", "todo", "merged", "cancelled"];

function StatusBadge({ status }: { status: Status }) {
  return (
    <Badge variant="outline" className={STATUS_TEXT[status.token]}>
      {status.label}
    </Badge>
  );
}

export function TaskBadge({ state }: { state: TaskState }) {
  return <StatusBadge status={taskStatus(state)} />;
}

export function PlanBadge({ state, paused }: { state: PlanState; paused?: boolean }) {
  return <StatusBadge status={planStatus(state, paused)} />;
}

export function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** A failed load: the error and a button that loads again. */
export function LoadError({ text, onRetry }: { text: string; onRetry: () => void }) {
  return (
    <div role="alert" className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-destructive">
      <span className="min-w-0">{text}</span>
      <Button variant="outline" size="sm" onClick={onRetry}>
        <RotateCwIcon /> Retry
      </Button>
    </div>
  );
}
