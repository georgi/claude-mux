import { describe, expect, it } from "vitest";
import { withCtrl } from "./keys";
import { formatRoute, parseRoute, type Route } from "./route";

describe("routes", () => {
  it("round-trip through the hash", () => {
    const all: Route[] = [
      { view: "home" },
      { view: "agent", agent: "P-20261018-demo/T-0002/tests", mode: "chat" },
      { view: "agent", agent: "P-20261018-demo/executor", mode: null },
      { view: "term", term: 3 },
      { view: "board", plan: "P-20261018-demo" },
      { view: "new" },
      { view: "settings" },
      { view: "needs" },
      { view: "plans" },
      { view: "live" },
    ];
    for (const r of all) expect(parseRoute(formatRoute(r))).toEqual(r);
  });

  it("falls back home on junk", () => {
    expect(parseRoute("")).toEqual({ view: "home" });
    expect(parseRoute("#/nope")).toEqual({ view: "home" });
    expect(parseRoute("#/term/x")).toEqual({ view: "home" });
    expect(parseRoute("#/agent/")).toEqual({ view: "home" });
  });
});

describe("withCtrl", () => {
  it("makes control codes from letters", () => {
    expect(withCtrl("c")).toBe("\x03");
    expect(withCtrl("C")).toBe("\x03");
    expect(withCtrl("[")).toBe("\x1b");
    expect(withCtrl(" ")).toBe("\x00");
    expect(withCtrl("1")).toBe("1");
    expect(withCtrl("ab")).toBe("ab");
  });
});
