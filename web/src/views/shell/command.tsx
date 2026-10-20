// The keyboard layer (one mount in App.tsx): the global key handler, the
// ⌘K palette and the `?` sheet. All three read `BINDINGS` in src/keys.ts.

import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import type { Api } from "@/api/http";
import type { StateView } from "@/api/types";
import { available, type Binding, BINDINGS, type KeyCtx, Matcher } from "@/keys";
import { type Nav, perform } from "@/needs";
import type { Route } from "@/route";
import { ShortcutsSheet } from "./shortcuts";

// cmdk and its Radix dialog load with the first ⌘K.
const Palette = lazy(() => import("./palette").then((m) => ({ default: m.Palette })));

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
  // Mounted from the first open on, so the palette can animate closed.
  const [opened, setOpened] = useState(false);
  if (palette && !opened) setOpened(true);
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
      {opened && (
        <Suspense fallback={null}>
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
        </Suspense>
      )}
      <ShortcutsSheet open={sheet} onOpenChange={setSheet} show={show} />
    </>
  );
}
