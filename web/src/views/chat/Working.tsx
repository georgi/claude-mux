import type { Activity } from "@/api/types";
import { formatDuration } from "@/chat";
import { useNow } from "@/lib/time";
import { Spinner } from "@/components/ui/spinner";

/** "Working…", "Waiting for permission", "Idle 4m", or nothing while the state is unknown. */
export function activityLabel(activity: Activity, idleSince: number | null, now: number): string {
  switch (activity) {
    case "working":
      return "Working…";
    case "needs_permission":
      return "Waiting for permission";
    case "idle": {
      const ms = idleSince === null ? 0 : now - idleSince;
      if (ms < 60_000) return "Idle";
      // "4m", "2h 5m": the seconds are noise here.
      return `Idle ${formatDuration(ms).replace(/ \d+s$/, "")}`;
    }
    default:
      return "";
  }
}

/**
 * The live row under the log. It keeps one height whatever it says, and it
 * sits outside the scrolled content, so it never moves the log.
 */
export function WorkingRow({ activity, idleSince }: { activity: Activity; idleSince: number | null }) {
  const now = useNow(15_000);
  const busy = activity === "working" || activity === "needs_permission";
  const label = activityLabel(activity, idleSince, now);
  return (
    <div
      role="status"
      aria-label="Agent activity"
      className="flex h-7 shrink-0 items-center gap-2 px-4 text-xs text-muted-foreground"
    >
      {busy && <Spinner role="presentation" aria-label={undefined} aria-hidden="true" className="size-3.5" />}
      <span className={busy ? "shimmer motion-reduce:shimmer-none" : undefined}>{label}</span>
    </div>
  );
}
