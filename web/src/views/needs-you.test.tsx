// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Toaster } from "sonner";
import { afterEach, describe, expect, it } from "vitest";
import type { ItemKind, StateView } from "../api/types";
import { agent, fakeApi, item, PLAN, plan, state, task } from "../fixtures";
import { type Nav, NUDGE } from "../needs";
import { idleFor, markAnswered, NeedsYou, useAttention } from "./NeedsYou";

afterEach(() => {
  cleanup();
  localStorage.clear();
  document.title = "cmux";
});

const nav: Nav = { openAgent: () => {}, openPlan: () => {}, openUrl: () => {} };
const perm: ItemKind = { permission: { tool: "Bash", input_summary: "cargo test -p perms" } };

const view = (kind: ItemKind, id = 5, over: Partial<StateView> = {}) =>
  state({ agents: [agent("T-0004")], items: [item(id, { kind, from: `${PLAN}/T-0004`, created: 0 })], ...over });

const card = (name: RegExp | string) => screen.getByRole("listitem", { name });

function deferred() {
  let fail!: (e: Error) => void;
  const promise = new Promise<void>((_, rej) => (fail = rej));
  return { promise, fail };
}

describe("NeedsYou cards", () => {
  it("shows a permission with its tool, input and actions, and Allow answers it", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view(perm)} api={api} nav={nav} now={120_000} />);
    const c = card("Allow Bash?");
    expect(c.querySelector("pre")?.textContent).toBe("cargo test -p perms");
    expect(c.textContent).toContain("2m");
    expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual(["Allow", "Always", "Deny…", "Open"]);
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(api.calls).toHaveLength(1));
    const [name, id, req, key] = api.calls[0];
    expect([name, id, req]).toEqual(["answer", 5, { action: "allow", text: null }]);
    expect(key).toMatch(/^[0-9a-f-]{36}$/);
  });

  it("denies with a note", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view(perm)} api={api} nav={nav} />);
    fireEvent.click(screen.getByRole("button", { name: "Deny…" }));
    fireEvent.input(screen.getByLabelText("Note"), { target: { value: "not on main" } });
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "deny", text: "not on main" }]));
  });

  it("replies to a question from its textarea with ⌘↵", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view("question")} api={api} nav={nav} />);
    expect(card("Question").textContent).toContain("which db?");
    const send = screen.getByRole("button", { name: "Reply" }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    const box = screen.getByLabelText("Reply");
    fireEvent.input(box, { target: { value: "postgres" } });
    expect(send.disabled).toBe(false);
    fireEvent.keyDown(box, { key: "Enter", metaKey: true });
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "reply", text: "postgres" }]));
  });

  it("shows a plan review with its title and task count, and accepts it", async () => {
    const api = fakeApi();
    const v = state({
      plans: [plan({ tasks: ["T-0001", "T-0002", "T-0003"] })],
      items: [item(5, { kind: "plan_review", from: `${PLAN}/executor` })],
    });
    render(<NeedsYou view={v} api={api} nav={nav} />);
    expect(card("Plan ready for review: demo").textContent).toContain("3 tasks");
    expect(screen.getByRole("button", { name: "Send back…" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Accept" }));
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "accept", text: null }]));
  });

  it("shows how long a stalled agent idled, and nudges it", async () => {
    const api = fakeApi();
    const v = view("stalled", 5, { agents: [agent("T-0004", { idle_since: 0, stalled: true })] });
    render(<NeedsYou view={v} api={api} nav={nav} now={(3 * 60 + 5) * 60_000} />);
    expect(card("Stalled").textContent).toContain("Idle for 3h 5m");
    expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual(["Open", "Nudge", "Dismiss"]);
    fireEvent.click(screen.getByRole("button", { name: "Nudge" }));
    await waitFor(() =>
      expect(api.calls.map((c) => c.slice(0, 3))).toEqual([
        ["message", `${PLAN}/T-0004`, NUDGE],
        ["answer", 5, { action: "dismiss", text: null }],
      ]),
    );
  });

  it("links the PR of a failing CI and opens the agent", () => {
    const seen: string[] = [];
    const v = state({
      agents: [agent("T-0004")],
      tasks: [task("T-0004", { pr: { url: "https://github.com/o/r/pull/41", number: 41, ci: "failing", ci_failures: 2 } })],
      items: [item(5, { kind: "ci_failing", from: `${PLAN}/T-0004`, body: "CI failing" })],
    });
    render(<NeedsYou view={v} api={fakeApi()} nav={{ ...nav, openAgent: (a) => seen.push(a) }} />);
    expect(screen.getByRole("link", { name: "#41" }).getAttribute("href")).toBe("https://github.com/o/r/pull/41");
    fireEvent.click(screen.getByRole("button", { name: "Open agent" }));
    expect(seen).toEqual([`${PLAN}/T-0004`]);
  });

  it("disables answers on a read-only device but still opens", () => {
    render(<NeedsYou view={view(perm)} api={fakeApi()} nav={nav} readOnly />);
    expect((screen.getByRole("button", { name: "Allow" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Open" }) as HTMLButtonElement).disabled).toBe(false);
  });
});

describe("NeedsYou list", () => {
  it("groups by plan, newest first", () => {
    const OTHER = "P-20261018-other";
    const v = state({
      plans: [plan(), plan({ id: OTHER, title: "other plan" })],
      items: [
        item(1, { created: 10, body: "old" }),
        item(2, { created: 30, from: `${OTHER}/T-0001`, body: "newest" }),
        item(3, { created: 20, body: "newer" }),
      ],
    });
    render(<NeedsYou view={v} api={fakeApi()} nav={nav} />);
    const lists = screen.getAllByRole("list");
    expect(lists.map((l) => l.getAttribute("aria-label"))).toEqual(["other plan", "demo"]);
    expect(screen.getAllByRole("listitem").map((c) => c.getAttribute("data-item"))).toEqual(["2", "3", "1"]);
  });

  it("moves the focus with j and k and shows key hints on the focused card", () => {
    const v = state({ items: [item(1, { created: 20 }), item(2, { kind: perm, created: 10 })] });
    render(<NeedsYou view={v} api={fakeApi()} nav={nav} />);
    const [q, p] = screen.getAllByRole("listitem");
    expect([q.tabIndex, p.tabIndex]).toEqual([0, -1]);
    act(() => q.focus());
    fireEvent.keyDown(q, { key: "j" });
    expect(document.activeElement).toBe(p);
    expect(p.tabIndex).toBe(0);
    expect([...p.querySelectorAll("kbd[data-slot=kbd]")].map((k) => k.textContent)).toContain("A");
    expect(q.querySelector("kbd")).toBeNull();
    fireEvent.keyDown(p, { key: "k" });
    expect(document.activeElement).toBe(q);
  });

  it("allows a focused permission with the a key", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view(perm)} api={api} nav={nav} />);
    const c = card("Allow Bash?");
    act(() => c.focus());
    fireEvent.keyDown(c, { key: "a" });
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "allow", text: null }]));
  });

  it("answers Always, Deny and Enter from the keyboard", async () => {
    const run = async (key: string, shiftKey = false) => {
      const api = fakeApi();
      render(<NeedsYou view={view(perm)} api={api} nav={nav} />);
      const c = card("Allow Bash?");
      act(() => c.focus());
      fireEvent.keyDown(c, { key, shiftKey });
      await waitFor(() => expect(api.calls).toHaveLength(1));
      cleanup();
      return api.calls[0][2];
    };
    expect(await run("A", true)).toEqual({ action: "allow_always", text: null });
    expect(await run("d")).toEqual({ action: "deny", text: null });
    expect(await run("Enter")).toEqual({ action: "allow", text: null });
  });

  it("removes an answered card at once and restores it with a toast on error", async () => {
    const d = deferred();
    const api = fakeApi({ answer: () => d.promise });
    render(
      <>
        <NeedsYou view={view(perm)} api={api} nav={nav} />
        <Toaster />
      </>,
    );
    const c = card("Allow Bash?");
    act(() => c.focus());
    fireEvent.keyDown(c, { key: "a" });
    // Collapsing at once, then gone after the exit animation.
    expect(c.getAttribute("data-leaving")).toBe("true");
    await waitFor(() => expect(screen.queryAllByRole("listitem", { hidden: true })).toHaveLength(0));
    expect(screen.getByText("Nothing needs you")).toBeTruthy();
    await act(async () => d.fail(new Error("I-5 was already answered by c-2")));
    const back = await screen.findByRole("listitem", { name: "Allow Bash?" });
    expect(back.getAttribute("data-leaving")).toBeNull();
    expect(await screen.findByText("Allow failed")).toBeTruthy();
    expect(await screen.findByText("I-5 was already answered by c-2")).toBeTruthy();
  });

  it("opens the agent with o without removing the card", () => {
    const seen: string[] = [];
    render(<NeedsYou view={view(perm)} api={fakeApi()} nav={{ ...nav, openAgent: (a) => seen.push(a) }} />);
    const c = card("Allow Bash?");
    act(() => c.focus());
    fireEvent.keyDown(c, { key: "o" });
    expect(seen).toEqual([`${PLAN}/T-0004`]);
    expect(c.getAttribute("data-leaving")).toBeNull();
  });

  it("opens the reply box with r", () => {
    render(<NeedsYou view={view("question")} api={fakeApi()} nav={nav} />);
    const c = card("Question");
    act(() => c.focus());
    fireEvent.keyDown(c, { key: "r" });
    expect(document.activeElement).toBe(screen.getByLabelText("Reply"));
  });
});

describe("NeedsYou empty", () => {
  it("says nothing needs you", () => {
    render(<NeedsYou view={state()} api={fakeApi()} nav={nav} />);
    expect(screen.getByText("Nothing needs you")).toBeTruthy();
    expect(screen.queryAllByRole("listitem")).toHaveLength(0);
  });

  it("shows when the last item was answered", () => {
    const at = new Date(2026, 9, 20, 14, 32).getTime();
    markAnswered(at);
    render(<NeedsYou view={state()} api={fakeApi()} nav={nav} />);
    const time = screen.getByText((_, el) => el?.tagName === "TIME");
    expect(time.getAttribute("datetime")).toBe(new Date(at).toISOString());
    expect(screen.getByText(/Last answered at/)).toBeTruthy();
  });

  it("formats idle times", () => {
    expect([idleFor(5_000), idleFor(12 * 60_000), idleFor(3 * 3_600_000), idleFor(50 * 3_600_000)]).toEqual([
      "5s",
      "12m",
      "3h",
      "2d",
    ]);
  });
});

describe("useAttention", () => {
  function Harness({ v, inbox, onOpen = () => {} }: { v: StateView; inbox: boolean; onOpen?: () => void }) {
    useAttention(v, inbox, onOpen);
    return <Toaster />;
  }

  it("puts the count in the title", () => {
    const { rerender } = render(<Harness v={view(perm)} inbox />);
    expect(document.title).toBe("(1) cmux");
    rerender(<Harness v={state()} inbox />);
    expect(document.title).toBe("cmux");
  });

  it("toasts a new item on another view, with an Open action", async () => {
    let opened = 0;
    const { rerender } = render(<Harness v={state()} inbox={false} onOpen={() => opened++} />);
    rerender(<Harness v={view(perm)} inbox={false} onOpen={() => opened++} />);
    expect(await screen.findByText("Allow Bash?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(opened).toBe(1);
  });

  it("does not toast in the inbox, for items already there, or for kinds turned off", async () => {
    // Sonner keeps toasts across tests: a tool no other test uses.
    const write: ItemKind = { permission: { tool: "Write", input_summary: "a.txt" } };
    localStorage.setItem("cmux.notify", JSON.stringify({ permission: false }));
    const { rerender } = render(<Harness v={view(write)} inbox={false} />);
    rerender(<Harness v={view(write, 6)} inbox={false} />);
    rerender(<Harness v={view("question", 7)} inbox />);
    rerender(<Harness v={view("question", 8)} inbox={false} />);
    expect(await screen.findByText("Question")).toBeTruthy();
    expect(screen.queryByText("Allow Write?")).toBeNull();
  });
});
