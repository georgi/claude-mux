import type { StatusToken } from "@/lib/status";

// Literal class strings per status token, so Tailwind finds them.

export const STATUS_BG: Record<StatusToken, string> = {
  working: "bg-status-working",
  idle: "bg-status-idle",
  needs: "bg-status-needs",
  stalled: "bg-status-stalled",
  failing: "bg-status-failing",
  passing: "bg-status-passing",
  merged: "bg-status-merged",
  blocked: "bg-status-blocked",
  cancelled: "bg-status-cancelled",
};

/** A graph node: a light tint of the status with a status-colored border. */
export const STATUS_NODE: Record<StatusToken, string> = {
  working: "fill-status-working/12 stroke-status-working",
  idle: "fill-status-idle/12 stroke-status-idle",
  needs: "fill-status-needs/12 stroke-status-needs",
  stalled: "fill-status-stalled/12 stroke-status-stalled",
  failing: "fill-status-failing/12 stroke-status-failing",
  passing: "fill-status-passing/12 stroke-status-passing",
  merged: "fill-status-merged/12 stroke-status-merged",
  blocked: "fill-status-blocked/12 stroke-status-blocked",
  cancelled: "fill-status-cancelled/12 stroke-status-cancelled",
};
