// @vitest-environment happy-dom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { TokenUsage } from "../api/types";
import { agent, plan, PLAN, state, task } from "../fixtures";
import { ContextMeter, contextPercent } from "./shell/context-meter";
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

describe("ContextMeter", () => {
  // 30k input + 60k cache read + 10k cache write = 100k of 200k; output does not count.
  const tokens: TokenUsage = { input: 30_000, output: 9_000, cache_read: 60_000, cache_write: 10_000 };

  it("counts input and cache tokens against a 200k window", () => {
    expect(contextPercent(tokens)).toBe(50);
    expect(contextPercent({ input: 250_000, output: 0, cache_read: 0, cache_write: 0 })).toBe(100);
  });

  it("shows the percentage as a meter", () => {
    render(<ContextMeter tokens={tokens} />);
    const meter = screen.getByRole("meter", { name: "Context" });
    expect(meter.getAttribute("aria-valuenow")).toBe("50");
    expect(screen.getByText("50%")).toBeTruthy();
  });
});
