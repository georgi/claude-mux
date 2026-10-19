import {
  CheckIcon,
  CircleHelpIcon,
  CircleIcon,
  DiamondIcon,
  FlagIcon,
  Loader2Icon,
  SquareIcon,
  SquareTerminalIcon,
  TriangleAlertIcon,
  XIcon,
} from "lucide-react";
import { cn } from "cn";

const LABELS: Record<string, string> = {
  "✓": "Done",
  "✗": "Failed",
  "■": "Stopped",
  "○": "Not started",
  "!": "Stalled",
  "⚑": "Needs permission",
  "◆": "Idle",
  "?": "Unknown",
  "▸": "Terminal",
};

/** Small status icon for a glyph from src/tree.ts. Colors are theme tokens. */
export function StatusIcon({ glyph, className }: { glyph: string; className?: string }) {
  const working = /^[⠀-⣿]$/.test(glyph);
  const label = working ? "Working" : (LABELS[glyph] ?? "Unknown");
  const cls = cn("size-3.5 shrink-0", className);
  let icon: React.ReactNode;
  if (working) icon = <Loader2Icon className={cn(cls, "animate-spin text-primary")} />;
  else
    switch (glyph) {
      case "✓":
        icon = <CheckIcon className={cn(cls, "text-foreground")} />;
        break;
      case "✗":
        icon = <XIcon className={cn(cls, "text-destructive")} />;
        break;
      case "■":
        icon = <SquareIcon className={cn(cls, "text-muted-foreground")} />;
        break;
      case "○":
        icon = <CircleIcon className={cn(cls, "text-muted-foreground")} />;
        break;
      case "!":
        icon = <TriangleAlertIcon className={cn(cls, "text-destructive")} />;
        break;
      case "⚑":
        icon = <FlagIcon className={cn(cls, "text-primary")} />;
        break;
      case "◆":
        icon = <DiamondIcon className={cn(cls, "text-muted-foreground")} />;
        break;
      case "▸":
        icon = <SquareTerminalIcon className={cn(cls, "text-muted-foreground")} />;
        break;
      default:
        icon = <CircleHelpIcon className={cn(cls, "text-muted-foreground")} />;
    }
  return (
    <span role="img" aria-label={label} title={label} className="inline-flex">
      {icon}
    </span>
  );
}
