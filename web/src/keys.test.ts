import { describe, expect, it } from "vitest";
import { agent, PLAN, plan, state } from "./fixtures";
import {
  available,
  BINDINGS,
  type Binding,
  chordMatches,
  displayKeys,
  isMac,
  isTyping,
  type KeyCtx,
  type KeyEv,
  Matcher,
  parseChord,
  SEQUENCE_MS,
  stepAgent,
  type Target,
} from "./keys";

const ev = (key: string, mods: Partial<Omit<KeyEv, "key">> = {}): KeyEv => ({
  key,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

const b = (id: string, ...keys: string[]): Binding => ({ id, keys, label: id, group: "General", run: () => {} });

const el = (tagName: string, opts: { editable?: boolean; xterm?: boolean } = {}): Target => ({
  tagName,
  isContentEditable: opts.editable ?? false,
  closest: (sel: string) => (sel === ".xterm" && opts.xterm ? {} : null),
});
const body = el("BODY");

describe("platform", () => {
  it("tells a Mac from the platform string", () => {
    expect(isMac("MacIntel")).toBe(true);
    expect(isMac("iPhone")).toBe(true);
    expect(isMac("Linux x86_64")).toBe(false);
    expect(isMac("Win32")).toBe(false);
  });

  it("maps mod to ⌘ on a Mac and to Ctrl elsewhere", () => {
    const mac = parseChord("mod+k", true);
    const pc = parseChord("mod+k", false);
    expect(chordMatches(mac, ev("k", { metaKey: true }))).toBe(true);
    expect(chordMatches(mac, ev("k", { ctrlKey: true }))).toBe(false);
    expect(chordMatches(pc, ev("k", { ctrlKey: true }))).toBe(true);
    expect(chordMatches(pc, ev("k", { metaKey: true }))).toBe(false);
  });

  it("shows ⌘ on a Mac and Ctrl elsewhere", () => {
    expect(displayKeys("mod+k", true)).toEqual([["⌘", "K"]]);
    expect(displayKeys("mod+k", false)).toEqual([["Ctrl", "K"]]);
    expect(displayKeys("g n", true)).toEqual([["g"], ["n"]]);
  });

  it("opens the palette with Ctrl+K on a Mac too", () => {
    const m = new Matcher(BINDINGS, true);
    expect((m.feed(ev("k", { metaKey: true }), body, 0) as Binding).id).toBe("palette");
    expect((m.feed(ev("k", { ctrlKey: true }), body, 0) as Binding).id).toBe("palette");
    const pc = new Matcher(BINDINGS, false);
    expect((pc.feed(ev("k", { ctrlKey: true }), body, 0) as Binding).id).toBe("palette");
    expect(pc.feed(ev("k", { metaKey: true }), body, 0)).toBeNull();
  });
});

describe("modifiers", () => {
  it("needs the exact modifiers", () => {
    const c = parseChord("ctrl+shift+p", false);
    expect(chordMatches(c, ev("P", { ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(chordMatches(c, ev("p", { ctrlKey: true }))).toBe(false);
    expect(chordMatches(c, ev("p", { ctrlKey: true, shiftKey: true, altKey: true }))).toBe(false);
  });

  it("does not take a plain letter with a modifier held", () => {
    const m = new Matcher([b("j", "j")], false);
    expect(m.feed(ev("j", { ctrlKey: true }), body, 0)).toBeNull();
    expect(m.feed(ev("J", { shiftKey: true }), body, 0)).toBeNull();
    expect(m.feed(ev("j"), body, 0)).toMatchObject({ id: "j" });
  });

  it("takes ? with the Shift that types it", () => {
    const m = new Matcher([b("help", "?")], false);
    expect(m.feed(ev("?", { shiftKey: true }), body, 0)).toMatchObject({ id: "help" });
  });

  it("ignores a lone modifier press", () => {
    const m = new Matcher([b("help", "?")], false);
    expect(m.feed(ev("Shift", { shiftKey: true }), body, 0)).toBeNull();
  });
});

describe("sequences", () => {
  const m = () => new Matcher([b("needs", "g n"), b("plans", "g p")], false);

  it("completes g n and g p", () => {
    const x = m();
    expect(x.feed(ev("g"), body, 0)).toBe("pending");
    expect(x.feed(ev("n"), body, 100)).toMatchObject({ id: "needs" });
    expect(x.feed(ev("g"), body, 200)).toBe("pending");
    expect(x.feed(ev("p"), body, 300)).toMatchObject({ id: "plans" });
  });

  it("drops the prefix after a wrong key", () => {
    const x = m();
    x.feed(ev("g"), body, 0);
    expect(x.feed(ev("x"), body, 10)).toBeNull();
    expect(x.feed(ev("n"), body, 20)).toBeNull();
  });

  it("drops the prefix after the timeout", () => {
    const x = m();
    x.feed(ev("g"), body, 0);
    expect(x.feed(ev("n"), body, SEQUENCE_MS + 1)).toBeNull();
  });

  it("keeps the prefix over a Shift press", () => {
    const x = new Matcher([b("help", "g ?")], false);
    x.feed(ev("g"), body, 0);
    expect(x.feed(ev("Shift", { shiftKey: true }), body, 10)).toBe("pending");
    expect(x.feed(ev("?", { shiftKey: true }), body, 20)).toMatchObject({ id: "help" });
  });

  it("skips bindings the filter rules out", () => {
    const x = m();
    x.feed(ev("g"), body, 0);
    expect(x.feed(ev("n"), body, 10, (y) => y.id !== "needs")).toBeNull();
  });
});

describe("input focus", () => {
  it("knows fields and the terminal", () => {
    expect(isTyping(el("INPUT"))).toBe(true);
    expect(isTyping(el("textarea"))).toBe(true);
    expect(isTyping(el("SELECT"))).toBe(true);
    expect(isTyping(el("DIV", { editable: true }))).toBe(true);
    expect(isTyping(el("DIV", { xterm: true }))).toBe(true);
    expect(isTyping(el("BUTTON"))).toBe(false);
    expect(isTyping(null)).toBe(false);
  });

  it("ignores plain keys while typing", () => {
    const x = new Matcher(BINDINGS, false);
    for (const t of [el("INPUT"), el("TEXTAREA"), el("DIV", { xterm: true })]) {
      expect(x.feed(ev("g"), t, 0)).toBeNull();
      expect(x.feed(ev("?", { shiftKey: true }), t, 0)).toBeNull();
      expect(x.feed(ev("j"), t, 0)).toBeNull();
    }
  });

  it("drops a pending sequence when focus moves into a field", () => {
    const x = new Matcher(BINDINGS, false);
    expect(x.feed(ev("g"), body, 0)).toBe("pending");
    expect(x.feed(ev("n"), el("INPUT"), 10)).toBeNull();
  });

  it("opens the palette from a field, not from the terminal", () => {
    const x = new Matcher(BINDINGS, false);
    expect(x.feed(ev("k", { ctrlKey: true }), el("INPUT"), 0)).toMatchObject({ id: "palette" });
    expect(x.feed(ev("k", { ctrlKey: true }), el("DIV", { xterm: true }), 0)).toBeNull();
  });
});

describe("registry", () => {
  const v = state({
    plans: [plan({ agents: [`${PLAN}/executor`, `${PLAN}/T-0001`] })],
    agents: [agent("executor"), agent("T-0001", { parent: `${PLAN}/executor`, level: "task" })],
  });
  const ctx = (over: Partial<KeyCtx> = {}): KeyCtx => ({
    route: { view: "agent", agent: `${PLAN}/executor`, mode: null },
    view: v,
    readOnly: false,
    go: () => {},
    openPalette: () => {},
    openShortcuts: () => {},
    toggleTheme: () => {},
    focusComposer: () => {},
    signOut: () => {},
    stopAgent: () => {},
    resumeAgent: () => {},
    planAction: () => {},
    ...over,
  });

  it("has unique ids and parseable keys", () => {
    const ids = BINDINGS.map((x) => x.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const x of BINDINGS) for (const k of x.keys) expect(displayKeys(k).length).toBeGreaterThan(0);
  });

  it("has no chord that is the prefix of a sequence", () => {
    const all = BINDINGS.flatMap((x) => x.keys);
    for (const k of all) for (const o of all) if (o !== k) expect(o.startsWith(`${k} `)).toBe(false);
  });

  it("hides mutating actions on a read-only device", () => {
    const full = BINDINGS.filter((x) => available(x, ctx())).map((x) => x.id);
    const read = BINDINGS.filter((x) => available(x, ctx({ readOnly: true }))).map((x) => x.id);
    expect(full).toContain("stop-agent");
    expect(full).toContain("new-plan");
    expect(read).not.toContain("stop-agent");
    expect(read).not.toContain("new-plan");
    expect(read).toContain("chat");
  });

  it("steps through agents in tree order and wraps", () => {
    expect(stepAgent(v, null, 1)).toBe(`${PLAN}/executor`);
    expect(stepAgent(v, `${PLAN}/executor`, 1)).toBe(`${PLAN}/T-0001`);
    expect(stepAgent(v, `${PLAN}/T-0001`, 1)).toBe(`${PLAN}/executor`);
    expect(stepAgent(v, `${PLAN}/executor`, -1)).toBe(`${PLAN}/T-0001`);
  });

  it("runs g n to the needs-you view", () => {
    const went: unknown[] = [];
    const c = ctx({ go: (r) => went.push(r) });
    const x = new Matcher(BINDINGS, false);
    x.feed(ev("g"), body, 0, (y) => available(y, c));
    const hit = x.feed(ev("n"), body, 1, (y) => available(y, c));
    (hit as Binding).run(c);
    expect(went).toEqual([{ view: "needs" }]);
  });
});
