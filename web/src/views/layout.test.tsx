// @vitest-environment happy-dom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { TokenUsage } from "../api/types";
import { agent, plan, PLAN, state, task } from "../fixtures";
import { TokenUsageSummary } from "./shell/token-usage";
import { crumbsFor, Crumbs } from "./shell/header";

afterEach(cleanup);

const view = state({
  plans: [plan({ title: "Polish the web UI", agents: [`${PLAN}/executor`, `${PLAN}/T-0006`, `${PLAN}/T-0006/tests`] })],
  tasks: [task("T-0006", { title: "Layout" })],
  agents: [agent("executor"), agent("T-0006"), agent("T-0006/tests")],
});
const sub = { view: "agent", agent: `${PLAN}/T-0006/tests`, mode: null } as const;

describe("Breadcrumb", () => {
  it("links plan › task › sub-agent for a sub-agent route", () => {
    render(<Crumbs crumbs={crumbsFor(view, sub)} phone={false} />);
    const nav = screen.getByRole("navigation", { name: "breadcrumb" });
    const links = Array.from(nav.querySelectorAll("a")).map((a) => [a.textContent, a.getAttribute("href")]);
    expect(links).toEqual([
      ["Polish the web UI", `#/board/${PLAN}`],
      ["T-0006 Layout", `#/agent/${PLAN}/T-0006`],
    ]);
    const page = nav.querySelector('[aria-current="page"]');
    expect(page?.textContent).toBe("tests");
  });

  it("shows the last crumb and a back button on a phone", () => {
    render(<Crumbs crumbs={crumbsFor(view, sub)} phone />);
    const back = screen.getByRole("button", { name: "Back to T-0006 Layout" });
    expect(back.getAttribute("href")).toBe(`#/agent/${PLAN}/T-0006`);
    expect(screen.queryByText("Polish the web UI")).toBeNull();
    expect(screen.getByText("tests").getAttribute("aria-current")).toBe("page");
  });

  it("names the executor under its plan", () => {
    const crumbs = crumbsFor(view, { view: "agent", agent: `${PLAN}/executor`, mode: null });
    expect(crumbs.map((c) => c.label)).toEqual(["Polish the web UI", "Executor"]);
  });
});

describe("TokenUsageSummary", () => {
  it("labels all usage categories as cumulative tokens without claiming context occupancy", () => {
    const tokens: TokenUsage = { input: 30_000, output: 9_000, cache_read: 60_000, cache_write: 10_000 };
    render(<TokenUsageSummary tokens={tokens} />);
    expect(screen.getByRole("button", { name: "Cumulative token usage: 109,000 tokens" }).textContent).toBe("109k tokens total");
    expect(screen.queryByRole("meter")).toBeNull();
    expect(screen.queryByText(/%/)).toBeNull();
  });

  it("keeps displaying accumulated requests above a context window without capping them", () => {
    const { rerender } = render(<TokenUsageSummary tokens={{ input: 80_000, output: 0, cache_read: 0, cache_write: 0 }} />);
    expect(screen.getByRole("button", { name: "Cumulative token usage: 80,000 tokens" })).toBeTruthy();
    // Three requests of 80k each are 240k lifetime usage, not a full context window.
    rerender(<TokenUsageSummary tokens={{ input: 80_000, output: 0, cache_read: 160_000, cache_write: 0 }} />);
    expect(screen.getByRole("button", { name: "Cumulative token usage: 240,000 tokens" }).textContent).toBe("240k tokens total");
    expect(screen.queryByRole("meter")).toBeNull();
    expect(screen.queryByText(/%/)).toBeNull();
  });
});
