import { ChevronRightIcon, ListTreeIcon, LoaderCircleIcon } from "lucide-react";
import type { ToolGroup } from "@/chat";
import { formatDuration } from "@/chat";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";

/**
 * The Chain of Thought pattern: a run of tool calls under one line, "Worked
 * for N steps · 1m 12s". The running group stays open; a finished one folds.
 */
export function ToolGroupView({ group, children }: { group: ToolGroup; children: React.ReactNode }) {
  const label = group.running
    ? `Working · ${group.steps} steps${group.ms >= 1000 ? ` · ${formatDuration(group.ms)}` : ""}`
    : `Worked for ${group.steps} steps${group.ms >= 1000 ? ` · ${formatDuration(group.ms)}` : ""}`;
  return (
    // The key remounts the group when it finishes, so it folds then.
    <Collapsible key={group.running ? "running" : "done"} defaultOpen={group.running} className="flex min-w-0 flex-col">
      <CollapsibleTrigger className="group/cot flex w-fit items-center gap-2 rounded-md text-sm text-muted-foreground transition-colors outline-none hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50">
        {group.running ? (
          <LoaderCircleIcon className="size-4 animate-spin" aria-hidden="true" />
        ) : (
          <ListTreeIcon className="size-4" aria-hidden="true" />
        )}
        <span className={group.running ? "shimmer motion-reduce:shimmer-none" : undefined}>{label}</span>
        <ChevronRightIcon className="size-4 transition-transform group-aria-expanded/cot:rotate-90" aria-hidden="true" />
      </CollapsibleTrigger>
      <CollapsibleContent className="h-(--collapsible-panel-height) min-w-0 overflow-hidden transition-[height] duration-200 data-ending-style:h-0 data-starting-style:h-0">
        <div className="mt-2 ml-[calc(--spacing(2)-1px)] flex min-w-0 flex-col gap-3 border-l pl-4">{children}</div>
      </CollapsibleContent>
    </Collapsible>
  );
}
