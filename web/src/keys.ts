import type { StateView } from "./api/types";
import type { Route } from "./route";
import { agentOrder } from "./tree";

// The phone's terminal key bar (UI design 4.2): keys a phone keyboard lacks.
// `Ctrl` is sticky: it applies to the next key typed, then lets go.

export interface Key {
  label: string;
  /** Bytes sent to the PTY; absent for the `Ctrl` modifier. */
  data?: string;
  title: string;
}

export const KEYS: Key[] = [
  { label: "Esc", data: "\x1b", title: "Escape" },
  { label: "Tab", data: "\t", title: "Tab" },
  { label: "Ctrl", title: "Ctrl for the next key" },
  { label: "↑", data: "\x1b[A", title: "Up" },
  { label: "↓", data: "\x1b[B", title: "Down" },
  { label: "←", data: "\x1b[D", title: "Left" },
  { label: "→", data: "\x1b[C", title: "Right" },
  { label: "^C", data: "\x03", title: "Interrupt" },
  { label: "⏎", data: "\r", title: "Enter" },
  { label: "1", data: "1", title: "Option 1" },
  { label: "2", data: "2", title: "Option 2" },
  { label: "3", data: "3", title: "Option 3" },
];

/** `data` with Ctrl held: a letter (or `@[\]^_`) becomes its control code. */
export function withCtrl(data: string): string {
  if (data.length !== 1) return data;
  const c = data.toUpperCase().charCodeAt(0);
  if (c >= 0x40 && c <= 0x5f) return String.fromCharCode(c & 0x1f);
  if (data === " ") return "\x00";
  if (data === "?") return "\x7f";
  return data;
}

// ---------------------------------------------------------------------------
// Global shortcuts and the command palette. One registry (`BINDINGS`) feeds
// the key handler, the palette and the shortcuts sheet, so the three cannot
// drift. Matching is pure: it reads key events and an element-like target,
// so the tests drive it without a DOM.

export type Group = "Go to" | "Agent" | "Plan" | "General";
export const GROUPS: Group[] = ["Go to", "Agent", "Plan", "General"];

/** What a binding can see and do. App.tsx builds it from its state. */
export interface KeyCtx {
  route: Route;
  view: StateView | null;
  readOnly: boolean;
  go(r: Route): void;
  openPalette(): void;
  openShortcuts(): void;
  toggleTheme(): void;
  focusComposer(): void;
  signOut(): void;
  stopAgent(agent: string): void;
  resumeAgent(agent: string): void;
  planAction(plan: string, action: "accept" | "pause" | "resume"): void;
}

export interface Binding {
  id: string;
  /** Chords: `mod+k`, `g n` (a sequence), `?`. The first is the one shown. */
  keys: string[];
  label: string;
  group: Group;
  /** Changes server state: hidden on read-only devices. */
  mutates?: boolean;
  /** Absent from the palette (pure keyboard moves such as `j`). */
  hidden?: boolean;
  when?(c: KeyCtx): boolean;
  run(c: KeyCtx): void;
}

/** The agent the route shows, if any. */
export function currentAgent(c: Pick<KeyCtx, "route" | "view">) {
  const r = c.route;
  if (r.view !== "agent") return null;
  return c.view?.agents.find((a) => a.id === r.agent) ?? null;
}

/** The plan the route shows: the board's, or the open agent's. */
export function currentPlan(c: Pick<KeyCtx, "route" | "view">) {
  const r = c.route;
  const id = r.view === "board" ? r.plan : r.view === "agent" ? r.agent.split("/")[0] : null;
  return id ? (c.view?.plans.find((p) => p.id === id) ?? null) : null;
}

/** The agent `dir` steps from `current` in tree order, wrapping around. */
export function stepAgent(v: StateView, current: string | null, dir: 1 | -1): string | null {
  const ids = agentOrder(v);
  if (!ids.length) return null;
  const at = current ? ids.indexOf(current) : -1;
  if (at < 0) return dir === 1 ? ids[0] : ids[ids.length - 1];
  return ids[(at + dir + ids.length) % ids.length];
}

const live = (a: ReturnType<typeof currentAgent>) => !!a && (a.life === "running" || a.life === "starting");

export const BINDINGS: Binding[] = [
  { id: "palette", keys: ["mod+k", "ctrl+k"], label: "Command palette", group: "General", hidden: true, run: (c) => c.openPalette() },
  { id: "shortcuts", keys: ["?"], label: "Keyboard shortcuts", group: "General", run: (c) => c.openShortcuts() },
  { id: "needs", keys: ["g n"], label: "Needs you", group: "Go to", run: (c) => c.go({ view: "needs" }) },
  { id: "plans", keys: ["g p"], label: "Plans", group: "Go to", run: (c) => c.go({ view: "plans" }) },
  { id: "settings", keys: ["g s"], label: "Settings", group: "Go to", run: (c) => c.go({ view: "settings" }) },
  {
    id: "next-agent",
    keys: ["j"],
    label: "Next agent",
    group: "Go to",
    hidden: true,
    when: (c) => !!c.view?.agents.length,
    run: (c) => step(c, 1),
  },
  {
    id: "prev-agent",
    keys: ["k"],
    label: "Previous agent",
    group: "Go to",
    hidden: true,
    when: (c) => !!c.view?.agents.length,
    run: (c) => step(c, -1),
  },
  { id: "new-plan", keys: [], label: "New plan", group: "Plan", mutates: true, run: (c) => c.go({ view: "new" }) },
  {
    id: "chat",
    keys: ["c"],
    label: "Open chat",
    group: "Agent",
    when: (c) => !!currentAgent(c),
    run: (c) => c.go({ view: "agent", agent: currentAgent(c)!.id, mode: "chat" }),
  },
  {
    id: "terminal",
    keys: ["t"],
    label: "Open terminal",
    group: "Agent",
    when: (c) => currentAgent(c)?.term != null,
    run: (c) => c.go({ view: "agent", agent: currentAgent(c)!.id, mode: "terminal" }),
  },
  {
    id: "composer",
    keys: ["/"],
    label: "Focus the message box",
    group: "Agent",
    hidden: true,
    mutates: true,
    when: (c) => !!currentAgent(c),
    run: (c) => c.focusComposer(),
  },
  {
    id: "stop-agent",
    keys: [],
    label: "Stop agent",
    group: "Agent",
    mutates: true,
    when: (c) => live(currentAgent(c)),
    run: (c) => c.stopAgent(currentAgent(c)!.id),
  },
  {
    id: "resume-agent",
    keys: [],
    label: "Resume agent",
    group: "Agent",
    mutates: true,
    when: (c) => !!currentAgent(c) && !live(currentAgent(c)),
    run: (c) => c.resumeAgent(currentAgent(c)!.id),
  },
  {
    id: "accept-plan",
    keys: [],
    label: "Accept plan",
    group: "Plan",
    mutates: true,
    when: (c) => currentPlan(c)?.state === "proposed",
    run: (c) => c.planAction(currentPlan(c)!.id, "accept"),
  },
  {
    id: "pause-plan",
    keys: [],
    label: "Pause plan",
    group: "Plan",
    mutates: true,
    when: (c) => currentPlan(c)?.state === "accepted" && !currentPlan(c)!.paused,
    run: (c) => c.planAction(currentPlan(c)!.id, "pause"),
  },
  {
    id: "resume-plan",
    keys: [],
    label: "Resume plan",
    group: "Plan",
    mutates: true,
    when: (c) => !!currentPlan(c)?.paused,
    run: (c) => c.planAction(currentPlan(c)!.id, "resume"),
  },
  { id: "theme", keys: [], label: "Toggle theme", group: "General", run: (c) => c.toggleTheme() },
  { id: "sign-out", keys: [], label: "Sign out", group: "General", run: (c) => c.signOut() },
];

function step(c: KeyCtx, dir: 1 | -1) {
  if (!c.view) return;
  const cur = c.route.view === "agent" ? c.route.agent : null;
  const next = stepAgent(c.view, cur, dir);
  if (next) c.go({ view: "agent", agent: next, mode: c.route.view === "agent" ? c.route.mode : null });
}

/** Whether `b` applies now: its `when`, and read-only devices lose mutating ones. */
export function available(b: Binding, c: Pick<KeyCtx, "readOnly"> & Partial<KeyCtx>): boolean {
  if (b.mutates && c.readOnly) return false;
  return b.when ? b.when(c as KeyCtx) : true;
}

// --- Matching ---------------------------------------------------------------

/** The parts of a `KeyboardEvent` the matcher reads. */
export interface KeyEv {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

export function isMac(platform = typeof navigator === "undefined" ? "" : navigator.platform): boolean {
  return /mac|iphone|ipad|ipod/i.test(platform);
}

interface Chord {
  key: string;
  ctrl: boolean;
  meta: boolean;
  alt: boolean;
  shift: boolean;
}

/** `mod+shift+k` to a chord; `mod` is ⌘ on a Mac and Ctrl elsewhere. */
export function parseChord(s: string, mac: boolean): Chord {
  const parts = s.toLowerCase().split("+");
  // `+` itself as the key: `mod++`.
  const key = s.endsWith("++") ? "+" : parts[parts.length - 1];
  const mods = new Set(parts.slice(0, s.endsWith("++") ? -2 : -1));
  return {
    key,
    ctrl: mods.has("ctrl") || (!mac && mods.has("mod")),
    meta: mods.has("meta") || mods.has("cmd") || (mac && mods.has("mod")),
    alt: mods.has("alt"),
    shift: mods.has("shift"),
  };
}

export function chordMatches(ch: Chord, e: KeyEv): boolean {
  const key = e.key.toLowerCase();
  if (key !== ch.key) return false;
  if (e.ctrlKey !== ch.ctrl || e.metaKey !== ch.meta || e.altKey !== ch.alt) return false;
  // Shift makes `?` from `/`: for a symbol the key itself carries it.
  const letter = /^[a-z]$/.test(ch.key);
  return letter ? e.shiftKey === ch.shift : !ch.shift || e.shiftKey;
}

const hasMod = (e: KeyEv) => e.ctrlKey || e.metaKey || e.altKey;

/** An element-like target: enough to tell an editable field or the terminal. */
export interface Target {
  tagName?: string;
  isContentEditable?: boolean;
  closest?(sel: string): unknown;
}

/** Typing in a field or the terminal: plain keys are text, not shortcuts. */
export function isTyping(t: Target | null | undefined): boolean {
  if (!t) return false;
  const tag = (t.tagName ?? "").toUpperCase();
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || !!t.isContentEditable || inTerminal(t);
}

export function inTerminal(t: Target | null | undefined): boolean {
  return !!t?.closest?.(".xterm");
}

/** Within this many ms the second key of `g n` must come. */
export const SEQUENCE_MS = 1500;

/**
 * Feeds key events and returns the binding they complete. Holds the prefix of
 * a sequence (`g` of `g n`) between calls. Plain keys are ignored while
 * typing; modifier chords are ignored in the terminal only, where Ctrl keys
 * belong to the shell.
 */
export class Matcher {
  private prefix: string[] = [];
  private at = 0;

  constructor(
    private bindings: Binding[],
    private mac = isMac(),
  ) {}

  /** `binding`, `"pending"` (a sequence goes on), or `null`. */
  feed(e: KeyEv, target: Target | null, now: number, ok: (b: Binding) => boolean = () => true): Binding | "pending" | null {
    if (["shift", "control", "alt", "meta"].includes(e.key.toLowerCase())) return this.prefix.length ? "pending" : null;
    if (hasMod(e) ? inTerminal(target) : isTyping(target)) {
      this.prefix = [];
      return null;
    }
    if (this.prefix.length && now - this.at > SEQUENCE_MS) this.prefix = [];
    const seq = [...this.prefix];
    let hit: string | null = null;
    for (const b of this.bindings) {
      if (!ok(b)) continue;
      for (const k of b.keys) {
        const steps = k.split(" ").filter(Boolean);
        if (steps.length <= seq.length) continue;
        if (!seq.every((s, i) => s === steps[i])) continue;
        if (!chordMatches(parseChord(steps[seq.length], this.mac), e)) continue;
        if (steps.length === seq.length + 1) {
          this.prefix = [];
          return b;
        }
        hit = steps[seq.length];
      }
    }
    if (hit) {
      this.prefix = [...seq, hit];
      this.at = now;
      return "pending";
    }
    this.prefix = [];
    return null;
  }
}

/** A chord as the Kbd shows it: `⌘ K` on a Mac, `Ctrl K` elsewhere. */
export function displayKeys(keys: string, mac = isMac()): string[][] {
  return keys.split(" ").map((step) => {
    const c = parseChord(step, mac);
    const out: string[] = [];
    if (c.ctrl) out.push(mac ? "⌃" : "Ctrl");
    if (c.alt) out.push(mac ? "⌥" : "Alt");
    if (c.shift) out.push(mac ? "⇧" : "Shift");
    if (c.meta) out.push(mac ? "⌘" : "Meta");
    const named = c.key.length > 1 ? c.key[0].toUpperCase() + c.key.slice(1) : c.key;
    // A lone letter stays lower case (`j`); after a modifier it reads `⌘ K`.
    out.push(out.length && named.length === 1 ? named.toUpperCase() : named);
    return out;
  });
}
