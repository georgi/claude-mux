// The keyboard layer (one mount in App.tsx): the global key handler, the
// ⌘K palette and the `?` sheet. All three read `BINDINGS` in src/keys.ts.

import { useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
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
import type { Api } from "@/api/http";
import type { StateView } from "@/api/types";
import { available, type Binding, BINDINGS, type KeyCtx, Matcher } from "@/keys";
import { agentStatus, planStatus } from "@/lib/status";
import { cn } from "@/lib/utils";
import { type Nav, needs, perform } from "@/needs";
import type { Route } from "@/route";
import { buildTree } from "@/tree";
import { Keys, ShortcutsSheet } from "./shortcuts";
import { STATUS_TEXT } from "./status";

const RECENT = "cmux.recent";
const THEME = "cmux.theme";

function loadRecent(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(RECENT) ?? "[]") as unknown;
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function saveRecent(list: string[]) {
  try {
    localStorage.setItem(RECENT, JSON.stringify(list));
  } catch {
    // Private mode: recents last for this page only.
  }
}

/** Flips light and dark and keeps the choice (src/main.tsx reads it). */
export function toggleTheme() {
  const dark = document.documentElement.classList.toggle("dark");
  try {
    localStorage.setItem(THEME, dark ? "dark" : "light");
  } catch {
    // Private mode: the choice lasts for this page only.
  }
}

/** The order `ids` were last visited, most recent first; the rest keep theirs. */
function byRecent<T>(items: T[], key: (t: T) => string, recent: string[]): T[] {
  const rank = (t: T) => {
    const i = recent.indexOf(key(t));
    return i < 0 ? recent.length : i;
  };
  return items.map((t, i) => ({ t, i })).sort((a, b) => rank(a.t) - rank(b.t) || a.i - b.i).map((x) => x.t);
}

interface Props {
  view: StateView | null;
  route: Route;
  readOnly: boolean;
  api: Api;
  nav: Nav;
  go: (r: Route) => void;
  onSignOut: () => void;
}

export function Keyboard({ view, route, readOnly, api, nav, go, onSignOut }: Props) {
  const [palette, setPalette] = useState(false);
  const [sheet, setSheet] = useState(false);
  const [recent, setRecent] = useState(loadRecent);

  // Visiting an agent or a plan makes it recent.
  const target = route.view === "agent" ? `agent:${route.agent}` : route.view === "board" ? `plan:${route.plan}` : null;
  useEffect(() => {
    if (!target) return;
    setRecent((r) => {
      const next = [target, ...r.filter((x) => x !== target)].slice(0, 20);
      saveRecent(next);
      return next;
    });
  }, [target]);

  const act = (label: string, p: Promise<void>) =>
    p.then(
      () => toast.success(label),
      (e: unknown) => toast.error(e instanceof Error ? e.message : String(e)),
    );

  const ctx: KeyCtx = {
    route,
    view,
    readOnly,
    go,
    openPalette: () => setPalette(true),
    openShortcuts: () => setSheet(true),
    toggleTheme,
    focusComposer: () => document.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]')?.focus(),
    signOut: onSignOut,
    stopAgent: (a) => void act("Agent stopped", api.stopAgent(a)),
    resumeAgent: (a) => void act("Agent resumed", api.resumeAgent(a)),
    planAction: (p, a) =>
      void act({ accept: "Plan accepted", pause: "Plan paused", resume: "Plan resumed" }[a], api.plan(p, a)),
  };
  const ctxRef = useRef(ctx);
  ctxRef.current = ctx;

  useEffect(() => {
    const m = new Matcher(BINDINGS);
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      const t = e.target instanceof Element ? e.target : null;
      // Plain keys inside an open dialog or menu belong to it.
      const plain = !(e.ctrlKey || e.metaKey || e.altKey);
      if (plain && t?.closest('[role="dialog"],[role="alertdialog"],[role="menu"]')) return;
      const c = ctxRef.current;
      const hit = m.feed(e, t, performance.now(), (b) => available(b, c));
      if (hit === null) return;
      e.preventDefault();
      if (hit !== "pending") hit.run(c);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const show = (b: Binding) => available(b, ctx);

  return (
    <>
      <Palette
        open={palette}
        onOpenChange={setPalette}
        view={view}
        recent={recent}
        actions={BINDINGS.filter((b) => !b.hidden && show(b))}
        onAgent={nav.openAgent}
        onPlan={nav.openPlan}
        onNeed={(n) => void perform(api, nav, n, "open").catch((e: unknown) => toast.error(String(e)))}
        onAction={(b) => b.run(ctxRef.current)}
      />
      <ShortcutsSheet open={sheet} onOpenChange={setSheet} show={show} />
    </>
  );
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

function Palette({ open, onOpenChange, view, recent, actions, onAgent, onPlan, onNeed, onAction }: PaletteProps) {
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
