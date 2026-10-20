import type { PlanState, TaskState } from "../../api/types";
import { Badge } from "@/components/ui/badge";
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
