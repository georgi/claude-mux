// The ⌘K palette: agents, plans, needs and actions. A lazy chunk, because
// cmdk brings its own Radix dialog.

import { useMemo } from "react";
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@/components/ui/command";
import type { StateView } from "@/api/types";
import type { Binding } from "@/keys";
import { agentStatus, planStatus } from "@/lib/status";
import { cn } from "@/lib/utils";
import { needs } from "@/needs";
import { buildTree } from "@/tree";
import { Keys } from "./shortcuts";
import { STATUS_TEXT } from "./status";

/** The order `ids` were last visited, most recent first; the rest keep theirs. */
function byRecent<T>(items: T[], key: (t: T) => string, recent: string[]): T[] {
  const rank = (t: T) => {
    const i = recent.indexOf(key(t));
    return i < 0 ? recent.length : i;
  };
  return items.map((t, i) => ({ t, i })).sort((a, b) => rank(a.t) - rank(b.t) || a.i - b.i).map((x) => x.t);
}


interface PaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  view: StateView | null;
  recent: string[];
  actions: Binding[];
  onAgent: (agent: string) => void;
  onPlan: (plan: string) => void;
  onNeed: (n: ReturnType<typeof needs>[number]) => void;
  onAction: (b: Binding) => void;
}

export function Palette({ open, onOpenChange, view, recent, actions, onAgent, onPlan, onNeed, onAction }: PaletteProps) {
  const lists = useMemo(() => {
    if (!view) return { pending: [], agents: [], plans: [] };
    const order = buildTree(view).flatMap((r) => (r.agent ? [r.agent] : []));
    const tree = order.flatMap((id) => view.agents.filter((a) => a.id === id));
    return {
      pending: needs(view),
      agents: byRecent(tree, (a) => `agent:${a.id}`, recent),
      plans: byRecent(view.plans, (p) => `plan:${p.id}`, recent),
    };
  }, [view, recent]);

  // Closes first, then acts: the palette must not hold focus over the target.
  const pick = (f: () => void) => () => {
    onOpenChange(false);
    f();
  };

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Command palette" description="Go to an agent or a plan, or run an action.">
      <Command loop>
        <CommandInput placeholder="Agents, plans, actions…" aria-label="Command" />
        <CommandList>
          <CommandEmpty>No results.</CommandEmpty>
          {lists.pending.length > 0 && (
            <CommandGroup heading="Needs you">
              {lists.pending.map((n) => (
                <CommandItem
                  key={n.item.id}
                  value={`need-${n.item.id}`}
                  keywords={[n.title, n.who, n.detail]}
                  onSelect={pick(() => onNeed(n))}
                >
                  <span className="truncate">{n.title}</span>
                  <span className="ml-auto truncate text-xs text-muted-foreground">{n.who}</span>
                </CommandItem>
              ))}
            </CommandGroup>
          )}
          {lists.agents.length > 0 && (
            <CommandGroup heading="Agents">
              {lists.agents.map((a) => {
                const st = agentStatus(a);
                const task = a.task ? view?.tasks.find((t) => a.id.startsWith(`${t.plan}/`) && t.id === a.task) : undefined;
                return (
                  <CommandItem
                    key={a.id}
                    value={a.id}
                    keywords={[a.path, task?.title ?? ""]}
                    onSelect={pick(() => onAgent(a.id))}
                  >
                    <span aria-hidden className={cn("size-2 shrink-0 rounded-full bg-current", STATUS_TEXT[st.token])} />
                    <span className="font-mono text-xs">{a.path}</span>
                    {task && <span className="min-w-0 truncate text-muted-foreground">{task.title}</span>}
                    <span className="ml-auto shrink-0 text-xs text-muted-foreground">{st.label}</span>
                  </CommandItem>
                );
              })}
            </CommandGroup>
          )}
          {lists.plans.length > 0 && (
            <CommandGroup heading="Plans">
              {lists.plans.map((p) => {
                const st = planStatus(p.state, p.paused);
                return (
                  <CommandItem key={p.id} value={p.id} keywords={[p.title]} onSelect={pick(() => onPlan(p.id))}>
                    <span aria-hidden className={cn("size-2 shrink-0 rounded-full bg-current", STATUS_TEXT[st.token])} />
                    <span className="truncate">{p.title}</span>
                    <span className="ml-auto shrink-0 text-xs text-muted-foreground">{st.label}</span>
                  </CommandItem>
                );
              })}
            </CommandGroup>
          )}
          <CommandGroup heading="Actions">
            {actions.map((b) => (
              <CommandItem key={b.id} value={`action-${b.id}`} keywords={[b.label, b.group]} onSelect={pick(() => onAction(b))}>
                <span>{b.label}</span>
                {b.keys[0] && (
                  <CommandShortcut className="tracking-normal">
                    <Keys keys={b.keys[0]} />
                  </CommandShortcut>
                )}
              </CommandItem>
            ))}
          </CommandGroup>
        </CommandList>
      </Command>
    </CommandDialog>
  );
}
