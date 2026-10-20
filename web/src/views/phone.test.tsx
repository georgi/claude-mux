// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SidebarProvider } from "@/components/ui/sidebar";
import type { ItemKind } from "../api/types";
import { agent, fakeApi, item, PLAN, plan, state, task } from "../fixtures";
import { KEYS } from "../keys";
import type { Nav } from "../needs";
import { keepLogsAtBottom, readViewport } from "../pwa";
import { parseRoute } from "../route";
import { NeedsYou } from "./NeedsYou";
import { LONG_PRESS_MS, OfflineBanner, swipeThreshold } from "./shell/phone";
import { PhoneTabs } from "./shell/phone-tabs";
import { Tree } from "./Tree";

afterEach(cleanup);

const nav: Nav = { openAgent: () => {}, openPlan: () => {}, openUrl: () => {} };
const perm: ItemKind = { permission: { tool: "Bash", input_summary: "cargo test -p perms" } };
const view = (kind: ItemKind) =>
  state({ agents: [agent("T-0004")], items: [item(5, { kind, from: `${PLAN}/T-0004`, created: 0 })] });

/** A horizontal touch drag of `dx` px on `el`. */
function swipe(el: Element, dx: number) {
  const o = { pointerId: 1, pointerType: "touch", clientY: 100 };
  fireEvent.pointerDown(el, { ...o, clientX: 150 });
  fireEvent.pointerMove(el, { ...o, clientX: 150 + Math.sign(dx) * 20 });
  fireEvent.pointerMove(el, { ...o, clientX: 150 + dx });
  fireEvent.pointerUp(el, { ...o, clientX: 150 + dx });
}

const surface = (name: string) => screen.getByRole("listitem", { name }).querySelector("[data-slot=item]")!.parentElement!;

describe("PhoneTabs", () => {
  it("shows four tabs, the needs badge, and marks the current one", () => {
    const went: string[] = [];
    render(<PhoneTabs tab="agents" count={3} go={(t) => went.push(t)} />);
    const tabs = screen.getAllByRole("button");
    expect(tabs.map((b) => b.textContent)).toEqual(["3Needs you", "Agents", "Plans", "More"]);
    expect(screen.getByRole("button", { name: "Needs you, 3" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Agents" }).getAttribute("aria-current")).toBe("page");
    fireEvent.click(screen.getByRole("button", { name: "More" }));
    expect(went).toEqual(["more"]);
  });

  it("routes the Agents tab", () => {
    expect(parseRoute("#/agents")).toEqual({ view: "agents" });
  });
});

describe("swipe on a needs-you card", () => {
  it("arms at a third of the card's width, between 48 and 96 px", () => {
    expect(swipeThreshold(300)).toBe(96);
    expect(swipeThreshold(120)).toBe(48);
  });

  it("allows a permission on a swipe right past the threshold", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view(perm)} api={api} nav={nav} phone />);
    swipe(surface("Allow Bash?"), 30);
    expect(api.calls).toHaveLength(0);
    swipe(surface("Allow Bash?"), 140);
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "allow", text: null }]));
  });

  it("asks before it denies on a swipe left", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view(perm)} api={api} nav={nav} phone />);
    swipe(surface("Allow Bash?"), -140);
    expect(api.calls).toHaveLength(0);
    expect(await screen.findByText("The agent gets a refusal. This cannot be undone.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "deny", text: null }]));
  });

  it("dismisses a stalled agent on a swipe left, with no question", async () => {
    const api = fakeApi();
    const v = view("stalled");
    render(<NeedsYou view={v} api={api} nav={nav} phone />);
    const name = screen.getByRole("listitem").getAttribute("aria-label")!;
    swipe(surface(name), -140);
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "dismiss", text: null }]));
  });

  it("does not swipe without the phone layout", () => {
    const api = fakeApi();
    render(<NeedsYou view={view(perm)} api={api} nav={nav} />);
    swipe(surface("Allow Bash?"), 140);
    expect(api.calls).toHaveLength(0);
  });

  it("opens the card's details in a sheet on a tap", async () => {
    render(<NeedsYou view={view(perm)} api={fakeApi()} nav={nav} phone />);
    fireEvent.click(screen.getByRole("button", { name: "Details: Allow Bash?" }));
    const sheet = await screen.findByRole("dialog");
    expect(sheet.textContent).toContain("cargo test -p perms");
    expect([...sheet.querySelectorAll("button")].map((b) => b.textContent)).toContain("Allow");
  });
});

describe("Tree on a phone", () => {
  beforeEach(() => {
    (window as unknown as { happyDOM: { setViewport(v: object): void } }).happyDOM.setViewport({ width: 390, height: 844 });
  });
  afterEach(() => {
    (window as unknown as { happyDOM: { setViewport(v: object): void } }).happyDOM.setViewport({ width: 1024, height: 768 });
    vi.useRealTimers();
  });

  it("opens the row actions in a sheet on a long-press, and the press does not open the row", async () => {
    vi.useFakeTimers();
    const opened: string[] = [];
    render(
      <SidebarProvider>
        <Tree
          view={state({
            plans: [plan({ tasks: ["T-0001"], agents: [`${PLAN}/executor`, `${PLAN}/T-0001`] })],
            tasks: [task("T-0001", { title: "Add pagination" })],
            agents: [agent("executor"), agent("T-0001")],
          })}
          selected={null}
          api={fakeApi()}
          onOpenAgent={(a) => opened.push(a)}
          onOpenTerm={() => {}}
          onPlan={() => {}}
        />
      </SidebarProvider>,
    );
    const row = screen.getByRole("treeitem", { name: "T-0001 Add pagination" }).firstElementChild!;
    fireEvent.pointerDown(row, { pointerId: 1, pointerType: "touch", clientX: 10, clientY: 10 });
    act(() => vi.advanceTimersByTime(LONG_PRESS_MS + 10));
    fireEvent.pointerUp(row, { pointerId: 1, pointerType: "touch" });
    fireEvent.click(row);
    expect(opened).toEqual([]);
    vi.useRealTimers();
    const sheet = await screen.findByRole("dialog");
    expect(sheet.textContent).toContain("T-0001 Add pagination");
    fireEvent.click(screen.getByRole("button", { name: "Open chat" }));
    expect(opened).toEqual([`${PLAN}/T-0001`]);
  });
});

describe("the offline banner", () => {
  it("shows Retry while the link is down, and nothing while it is up", () => {
    const retry = vi.fn();
    const { rerender, container } = render(<OfflineBanner status="closed" seen onRetry={retry} />);
    expect(screen.getByRole("alert").textContent).toContain("Offline");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(retry).toHaveBeenCalledOnce();
    rerender(<OfflineBanner status="connecting" seen onRetry={retry} />);
    expect(screen.getByRole("alert").textContent).toContain("Reconnecting");
    rerender(<OfflineBanner status="connecting" seen={false} onRetry={retry} />);
    expect(container.textContent).toBe("");
    rerender(<OfflineBanner status="open" seen onRetry={retry} />);
    expect(container.textContent).toBe("");
  });
});

describe("the keyboard", () => {
  it("measures the keyboard from the visual viewport", () => {
    expect(readViewport({ innerHeight: 844, visualViewport: null })).toEqual({ height: 844, top: 0, keyboard: 0 });
    const vv = { height: 508, offsetTop: 0 } as VisualViewport;
    expect(readViewport({ innerHeight: 844, visualViewport: vv })).toEqual({ height: 508, top: 0, keyboard: 336 });
    // iOS scrolls the page up under the keyboard: the offset is not keyboard.
    const scrolled = { height: 508, offsetTop: 120 } as VisualViewport;
    expect(readViewport({ innerHeight: 844, visualViewport: scrolled }).keyboard).toBe(216);
  });

  it("keeps a log that sits at its bottom anchored there", () => {
    const root = document.createElement("div");
    const at = document.createElement("div");
    const up = document.createElement("div");
    for (const [el, top] of [
      [at, 600],
      [up, 100],
    ] as const) {
      el.setAttribute("role", "log");
      Object.defineProperty(el, "scrollHeight", { value: 1000, configurable: true });
      Object.defineProperty(el, "clientHeight", { value: 400, configurable: true });
      el.scrollTop = top;
      root.append(el);
    }
    const restore = keepLogsAtBottom(root);
    at.scrollTop = 300;
    up.scrollTop = 50;
    restore();
    expect(at.scrollTop).toBe(1000);
    expect(up.scrollTop).toBe(50);
  });

  it("has the shell keys in the terminal key bar", () => {
    expect(KEYS.map((k) => k.label).slice(0, 10)).toEqual(["Esc", "Tab", "Ctrl", "↑", "↓", "←", "→", "|", "~", "/"]);
  });
});
