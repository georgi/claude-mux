import {
  BanIcon,
  CheckIcon,
  CircleCheckIcon,
  CircleDashedIcon,
  CircleDotIcon,
  CircleHelpIcon,
  CircleIcon,
  CircleSlashIcon,
  CircleXIcon,
  DiamondIcon,
  FilePenIcon,
  FlagIcon,
  FlaskConicalIcon,
  GitMergeIcon,
  LoaderCircleIcon,
  PauseIcon,
  SquareIcon,
  SquareTerminalIcon,
  TriangleAlertIcon,
  XIcon,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import type { Status, StatusIconName, StatusToken } from "@/lib/status";

const ICONS: Record<StatusIconName, LucideIcon> = {
  Ban: BanIcon,
  Check: CheckIcon,
  Circle: CircleIcon,
  CircleCheck: CircleCheckIcon,
  CircleDashed: CircleDashedIcon,
  CircleDot: CircleDotIcon,
  CircleHelp: CircleHelpIcon,
  CircleSlash: CircleSlashIcon,
  CircleX: CircleXIcon,
  Diamond: DiamondIcon,
  FilePen: FilePenIcon,
  FlaskConical: FlaskConicalIcon,
  Flag: FlagIcon,
  GitMerge: GitMergeIcon,
  LoaderCircle: LoaderCircleIcon,
  Pause: PauseIcon,
  Square: SquareIcon,
  SquareTerminal: SquareTerminalIcon,
  TriangleAlert: TriangleAlertIcon,
  X: XIcon,
};

/** Text color class per token. Literal strings, so Tailwind finds them. */
export const STATUS_TEXT: Record<StatusToken, string> = {
  working: "text-status-working",
  idle: "text-status-idle",
  needs: "text-status-needs",
  stalled: "text-status-stalled",
  failing: "text-status-failing",
  passing: "text-status-passing",
  merged: "text-status-merged",
  blocked: "text-status-blocked",
  cancelled: "text-status-cancelled",
};

/** Small status icon in the status color, labelled for screen readers. */
export function StatusIcon({ status, className }: { status: Status; className?: string }) {
  const Icon = ICONS[status.icon];
  return (
    <span role="img" aria-label={status.label} title={status.label} className="inline-flex">
      <Icon
        aria-hidden
        className={cn(
          "size-3.5 shrink-0",
          STATUS_TEXT[status.token],
          status.icon === "LoaderCircle" && "animate-spin",
          className,
        )}
      />
    </span>
  );
}
