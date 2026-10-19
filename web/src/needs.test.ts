import { describe, expect, it } from "vitest";
import type { ItemKind } from "./api/types";
import { agent, fakeApi, item, PLAN, plan, state, task } from "./fixtures";
import { actionsFor, describe as describeItem, groups, type Nav, NUDGE, needs, notifyKind, pending, perform } from "./needs";

const perm: ItemKind = { permission: { tool: "Bash", input_summary: "cargo test -p api" } };

function nav() {
  const seen: string[] = [];
  const n: Nav = {
    openAgent: (a) => seen.push(`agent ${a}`),
    openPlan: (p) => seen.push(`plan ${p}`),
    openUrl: (u) => seen.push(`url ${u}`),
  };
  return { n, seen };
}

const ids = (k: ItemKind) => actionsFor(item(1, { kind: k })).map((a) => a.id);

describe("actionsFor", () => {
  it("offers the actions of UI design 2.3 for each kind", () => {
    expect(ids(perm)).toEqual(["allow", "allow_always", "deny", "open"]);
    expect(ids("question")).toEqual(["reply", "dismiss", "open"]);
    expect(ids("child_question")).toEqual(["reply", "dismiss", "open"]);
    expect(ids("plan_review")).toEqual(["accept", "send_back", "open"]);
    expect(ids("stalled")).toEqual(["open", "nudge", "dismiss"]);
    expect(ids("exited_without_report")).toEqual(["message", "resume", "dismiss", "open"]);
    expect(ids("ci_failing")).toEqual(["open_pr", "open", "dismiss"]);
    expect(ids("child_report")).toEqual(["dismiss", "open"]);
  });

  it("asks for text where the action needs it", () => {
    const a = actionsFor(item(1, { kind: perm }));
    expect(a.find((x) => x.id === "deny")?.text).toBe("note");
    expect(actionsFor(item(1, { kind: "question" }))[0].text).toBe("required");
    expect(actionsFor(item(1, { kind: "plan_review" })).find((x) => x.id === "send_back")?.text).toBe("note");
  });
});

describe("needs", () => {
  it("lists pending items for a person, newest first", () => {
    const v = state({
      items: [
        item(1, { created: 10 }),
        item(2, { created: 30 }),
        item(3, { created: 20, status: "dismissed" }),
        item(4, { created: 40, to: { agent: `${PLAN}/T-0001` }, kind: "user_message" }),
      ],
    });
    expect(pending(v).map((i) => i.id)).toEqual([2, 1]);
  });

  it("describes a permission by its tool and command", () => {
    const v = state({ agents: [agent("T-0004")], items: [item(7, { kind: perm, from: `${PLAN}/T-0004` })] });
    const [n] = needs(v);
    expect([n.title, n.detail, n.who]).toEqual(["Allow Bash?", "cargo test -p api", "T-0004"]);
    expect(n.agent?.id).toBe(`${PLAN}/T-0004`);
  });

  it("describes a plan review by its size", () => {
    const v = state({
      plans: [plan({ tasks: ["T-0001", "T-0002"] })],
      items: [item(1, { kind: "plan_review", from: `${PLAN}/executor` })],
    });
    const n = describeItem(v, v.items[0]);
    expect([n.title, n.detail]).toEqual(["Plan ready for review: demo", "2 tasks, 1 repo"]);
  });
});

describe("groups", () => {
  const OTHER = "P-20261018-other";
  const v = state({
    plans: [plan(), plan({ id: OTHER, title: "other plan" })],
    items: [
      item(1, { created: 10 }),
      item(2, { created: 50, from: `${OTHER}/T-0001` }),
      item(3, { created: 30 }),
      item(4, { created: 20, from: null }),
      item(5, { created: 40, from: `${OTHER}/executor` }),
      item(6, { created: 30, from: `${PLAN}/T-0002` }),
    ],
  });

  it("groups by plan, the group with the newest item first", () => {
    const g = groups(v);
    expect(g.map((x) => [x.plan, x.title])).toEqual([
      [OTHER, "other plan"],
      [PLAN, "demo"],
      [null, "Other"],
    ]);
  });

  it("orders each group newest first, the higher id first on a tie", () => {
    expect(groups(v).map((x) => x.needs.map((n) => n.item.id))).toEqual([[2, 5], [6, 3, 1], [4]]);
  });

  it("puts items from an unknown plan under Other", () => {
    const g = groups(state({ items: [item(1, { from: "P-gone/T-0001" })] }));
    expect(g.map((x) => [x.plan, x.title, x.needs.length])).toEqual([[null, "Other", 1]]);
  });

  it("is empty with nothing pending", () => {
    expect(groups(state({ items: [item(1, { status: "dismissed" })] }))).toEqual([]);
  });
});

describe("notifyKind", () => {
  it("maps items to the notification settings", () => {
    const k = (kind: ItemKind) => notifyKind(item(1, { kind }));
    expect([k(perm), k("child_question"), k("plan_review"), k("exited_without_report"), k("ci_failing"), k("child_report")]).toEqual([
      "permission",
      "question",
      "plan_review",
      "stalled",
      "ci_failing",
      null,
    ]);
  });
});

describe("perform", () => {
  const from = `${PLAN}/T-0001`;
  const run = async (kind: ItemKind, id: Parameters<typeof perform>[3], text?: string, extra = {}) => {
    const v = state({
      agents: [agent("T-0001")],
      tasks: [task("T-0001", { pr: { url: "https://github.com/o/r/pull/41", number: 41, ci: "failing", ci_failures: 3 } })],
      items: [item(9, { kind, from, ...extra })],
    });
    const api = fakeApi();
    const { n, seen } = nav();
    await perform(api, n, describeItem(v, v.items[0]), id, text);
    return { calls: api.calls, seen };
  };

  it("answers permissions with a fresh idempotency key each time", async () => {
    const a = await run(perm, "allow");
    const b = await run(perm, "allow");
    expect(a.calls).toEqual([["answer", 9, { action: "allow", text: null }, expect.any(String)]]);
    expect(a.calls[0][3]).not.toBe(b.calls[0][3]);
    expect((await run(perm, "allow_always")).calls[0].slice(0, 3)).toEqual(["answer", 9, { action: "allow_always", text: null }]);
    expect((await run(perm, "deny", " too risky ")).calls[0].slice(0, 3)).toEqual(["answer", 9, { action: "deny", text: "too risky" }]);
    expect((await run(perm, "deny", "")).calls[0][2]).toEqual({ action: "deny", text: null });
  });

  it("replies, accepts and sends back", async () => {
    expect((await run("question", "reply", "postgres")).calls[0][2]).toEqual({ action: "reply", text: "postgres" });
    await expect(run("question", "reply", "  ")).rejects.toThrow(/needs text/);
    expect((await run("plan_review", "accept")).calls[0][2]).toEqual({ action: "accept", text: null });
    expect((await run("plan_review", "send_back", "split T-0002")).calls[0][2]).toEqual({
      action: "send_back",
      text: "split T-0002",
    });
    expect((await run("question", "dismiss")).calls[0][2]).toEqual({ action: "dismiss", text: null });
  });

  it("acts on the agent, then dismisses the item", async () => {
    const m = await run("stalled", "message", "status?");
    expect(m.calls.map((c) => c.slice(0, 4))).toEqual([
      ["message", from, "status?", false],
      ["answer", 9, { action: "dismiss", text: null }, expect.any(String)],
    ]);
    expect((await run("stalled", "resume")).calls.map((c) => c[0])).toEqual(["resumeAgent", "answer"]);
    expect((await run("stalled", "nudge")).calls.map((c) => c.slice(0, 3))).toEqual([
      ["message", from, NUDGE],
      ["answer", 9, { action: "dismiss", text: null }],
    ]);
    expect((await run("exited_without_report", "stop")).calls.map((c) => c[0])).toEqual(["stopAgent", "answer"]);
    await expect(run("stalled", "message", "")).rejects.toThrow(/needs text/);
  });

  it("opens things without calling the server", async () => {
    const pr = await run("ci_failing", "open_pr");
    expect([pr.calls, pr.seen]).toEqual([[], ["url https://github.com/o/r/pull/41"]]);
    expect((await run(perm, "open")).seen).toEqual([`agent ${from}`]);
    expect((await run("plan_review", "open", "", { from: `${PLAN}/executor` })).seen).toEqual([`plan ${PLAN}`]);
  });
});
