import { describe, expect, it } from "vitest";
import type { PushPayload } from "./api/types";
import { answerFor, clickUrl, notification, parsePayload } from "./notify";
import { b64url, kindsOf } from "./push";

const p = (over: Partial<PushPayload>): PushPayload => ({ kind: "permission", item: 7, plan: null, summary: "Bash: cargo test", ...over });

describe("notification", () => {
  it("offers Allow and Deny for a permission request", () => {
    const n = notification(p({}));
    expect(n.title).toBe("Permission request");
    expect(n.options.body).toBe("Bash: cargo test");
    expect(n.options.tag).toBe("item-7");
    expect(n.options.data).toEqual(p({}));
    expect(n.options.actions.map((a) => a.action)).toEqual(["allow", "deny"]);
  });

  it("offers a text Reply for a question", () => {
    const n = notification(p({ kind: "question" }));
    expect(n.options.actions).toEqual([{ action: "reply", title: "Reply", type: "text", placeholder: "Answer" }]);
  });

  it("offers no actions for the other kinds", () => {
    for (const kind of ["plan_review", "stalled", "ci_failing"] as const) expect(notification(p({ kind })).options.actions).toEqual([]);
    const done = notification(p({ kind: "plan_done", item: null, plan: "P-1" }));
    expect(done.title).toBe("Plan finished");
    expect(done.options.tag).toBe("plan-P-1");
    expect(done.options.actions).toEqual([]);
  });

  it("parses only a payload with a kind", () => {
    expect(parsePayload(JSON.stringify(p({})))).toEqual(p({}));
    expect(parsePayload("not json")).toBeNull();
    expect(parsePayload("{}")).toBeNull();
  });
});

describe("taps", () => {
  it("answers Allow, Deny and a Reply with text", () => {
    expect(answerFor(p({}), "allow", null)).toEqual({ action: "allow", text: null });
    expect(answerFor(p({}), "deny", null)).toEqual({ action: "deny", text: null });
    expect(answerFor(p({ kind: "question" }), "reply", "yes")).toEqual({ action: "reply", text: "yes" });
  });

  it("opens the app for a plain tap or a Reply without text", () => {
    expect(answerFor(p({}), "", null)).toBeNull();
    expect(answerFor(p({ kind: "question" }), "reply", "")).toBeNull();
    expect(answerFor(p({ item: null }), "allow", null)).toBeNull();
    expect(clickUrl(p({}))).toBe("/#/needs");
    expect(clickUrl(p({ kind: "plan_done", item: null, plan: "P-1 x" }))).toBe("/#/board/P-1%20x");
  });
});

describe("push", () => {
  it("decodes URL-safe base64 with and without padding", () => {
    expect([...b64url("-_8")]).toEqual([0xfb, 0xff]);
    expect([...b64url("AQID")]).toEqual([1, 2, 3]);
    expect([...b64url("AQ")]).toEqual([1]);
    expect([...b64url("AQ==")]).toEqual([1]);
  });

  it("takes the kinds that are on, a missing one counting as on", () => {
    expect(kindsOf({})).toEqual(["permission", "question", "plan_review", "stalled", "ci_failing", "plan_done"]);
    expect(kindsOf({ question: false, stalled: false, plan_done: true })).toEqual(["permission", "plan_review", "ci_failing", "plan_done"]);
  });
});
