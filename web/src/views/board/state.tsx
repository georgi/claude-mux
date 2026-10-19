import type { PlanState, TaskState } from "../../api/types";
import { Badge } from "@/components/ui/badge";

type Variant = "default" | "secondary" | "destructive" | "outline" | "ghost";

const TASK_VARIANT: Record<TaskState, Variant> = {
  todo: "outline",
  in_progress: "default",
  testing: "secondary",
  failing: "destructive",
  passing: "secondary",
  merged: "default",
  blocked: "destructive",
  cancelled: "ghost",
};

const PLAN_VARIANT: Record<PlanState, Variant> = {
  draft: "outline",
  proposed: "secondary",
  accepted: "default",
  done: "secondary",
  cancelled: "ghost",
};

/** Task states in the order the board groups them. */
export const TASK_ORDER: TaskState[] = ["in_progress", "testing", "failing", "blocked", "passing", "todo", "merged", "cancelled"];

export function TaskBadge({ state }: { state: TaskState }) {
  return <Badge variant={TASK_VARIANT[state]}>{state.replace("_", " ")}</Badge>;
}

export function PlanBadge({ state, paused }: { state: PlanState; paused?: boolean }) {
  return <Badge variant={paused ? "outline" : PLAN_VARIANT[state]}>{paused ? "paused" : state}</Badge>;
}

export function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
