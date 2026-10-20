import { describe, expect, it } from "vitest";
import { type ChatRow, chatRows, dayLabel, formatDuration, groupRows, KEEP, lastToolKey, mergeEntries, plainText, quoted, toolKind, turnsOf } from "./chat";
import { entry } from "./fixtures";

describe("mergeEntries", () => {
  it("appends new entries and drops repeats by uuid", () => {
    const a = [entry("1"), entry("2")];
    const b = mergeEntries(a, [entry("2"), entry("3"), entry("3")]);
    expect(b.map((e) => e.uuid)).toEqual(["1", "2", "3"]);
    expect(mergeEntries(b, [entry("1")])).toBe(b);
    expect(mergeEntries(b, [])).toBe(b);
  });

  it("keeps the newest entries", () => {
    const many = Array.from({ length: KEEP + 5 }, (_, i) => entry(String(i)));
    const out = mergeEntries([], many);
    expect(out).toHaveLength(KEEP);
    expect(out[0].uuid).toBe("5");
  });
});

describe("chatRows", () => {
  it("folds each tool result into its call", () => {
    const rows = chatRows([
      entry("u", { role: "user", text: "Add pagination" }),
      entry("a", { text: "On it." }),
      entry("c1", { kind: "tool_call", text: "git push", tool: { name: "Bash", summary: "git push" } }),
      entry("c2", { kind: "tool_call", text: "{}", tool: { name: "Read", summary: "src/a.rs" } }),
      entry("r1", { role: "user", kind: "tool_result", text: "pushed", tool: { name: "Bash", summary: "git push" } }),
      entry("r2", { role: "user", kind: "tool_result", text: "fn a()", tool: { name: "Read", summary: "src/a.rs" } }),
      entry("a2", { text: "Done." }),
    ]);
    expect(rows).toEqual([
      { kind: "user", key: "u", text: "Add pagination", ts: 1000 },
      { kind: "assistant", key: "a", text: "On it.", ts: 1000 },
      { kind: "tool", key: "c1", name: "Bash", summary: "git push", result: "pushed", isError: false, resultKey: "r1", ts: 1000, endTs: 1000 },
      { kind: "tool", key: "c2", name: "Read", summary: "src/a.rs", result: "fn a()", isError: false, resultKey: "r2", ts: 1000, endTs: 1000 },
      { kind: "assistant", key: "a2", text: "Done.", ts: 1000 },
    ]);
  });

  it("shows a running call without a result, and a result without a call on its own", () => {
    const rows = chatRows([
      entry("r", { role: "user", kind: "tool_result", text: "orphan" }),
      entry("c", { kind: "tool_call", text: "x", tool: { name: "Bash", summary: "sleep 9" } }),
    ]);
    expect(rows).toEqual([
      { kind: "tool", key: "r", name: "tool", summary: "", result: "orphan", isError: false, resultKey: "r", ts: 1000, endTs: 1000 },
      { kind: "tool", key: "c", name: "Bash", summary: "sleep 9", result: null, isError: false, ts: 1000 },
    ]);
  });

  it("pairs results with their call by call_id", () => {
    const bash = { name: "Bash", summary: "cargo test" };
    const rows = chatRows([
      entry("c1", { kind: "tool_call", text: "cargo test", tool: bash, call_id: "toolu_1" }),
      entry("c2", { kind: "tool_call", text: "cargo test", tool: bash, call_id: "toolu_2" }),
      // The first call's result arrives last: the name alone would pick c2.
      entry("r2", { role: "user", kind: "tool_result", text: "two", tool: bash, call_id: "toolu_2" }),
      entry("r1", {
        role: "user",
        kind: "tool_result",
        text: "one…",
        tool: bash,
        call_id: "toolu_1",
        is_error: true,
        full_len: 900,
      }),
      // An id with no call left (it fell off the top) does not take another call.
      entry("c3", { kind: "tool_call", text: "ls", tool: bash, call_id: "toolu_3" }),
      entry("rx", { role: "user", kind: "tool_result", text: "gone", tool: bash, call_id: "toolu_0" }),
    ]);
    expect(rows.map((r) => r.kind === "tool" && [r.key, r.result, r.isError, r.resultLen])).toEqual([
      ["c1", "one…", true, 900],
      ["c2", "two", false, undefined],
      ["c3", null, false, undefined],
      ["rx", "gone", false, undefined],
    ]);
  });

  it("keeps the structured input, the model and, when asked, thinking", () => {
    const input = { file_path: "/a.rs", old_string: "a", new_string: "b" };
    const entries = [
      entry("t", { kind: "thinking", text: "hmm" }),
      entry("a", { text: "Edit it.", model: "claude-opus-5-5" }),
      entry("c", { kind: "tool_call", text: "/a.rs", tool: { name: "Edit", summary: "/a.rs", input }, call_id: "x" }),
    ];
    expect(chatRows(entries).map((r) => r.kind)).toEqual(["assistant", "tool"]);
    const rows = chatRows(entries, { thinking: true });
    expect(rows[0]).toEqual({ kind: "thinking", key: "t", text: "hmm", ts: 1000, ms: 0 });
    expect(rows[1]).toEqual({ kind: "assistant", key: "a", text: "Edit it.", ts: 1000, model: "claude-opus-5-5" });
    expect(rows[2].kind === "tool" && rows[2].input).toEqual(input);
  });
});

describe("groupRows", () => {
  const tool = (key: string, ts: number, endTs: number | null = ts + 1000): ChatRow => ({
    kind: "tool",
    key,
    name: "Bash",
    summary: key,
    result: endTs === null ? null : "ok",
    isError: false,
    ts,
    ...(endTs === null ? {} : { endTs }),
  });
  const text = (key: string): ChatRow => ({ kind: "assistant", key, text: key, ts: 0 });
  const shape = (rows: ChatRow[]) =>
    groupRows(rows).map((i) => (i.kind === "group" ? `[${i.rows.map((r) => r.key).join(" ")}]` : i.key));

  it("folds a run of three or more consecutive tool rows", () => {
    const items = groupRows([text("a"), tool("t1", 0), tool("t2", 10_000), tool("t3", 20_000, 72_000), text("b")]);
    expect(items.map((i) => i.kind)).toEqual(["assistant", "group", "assistant"]);
    const g = items[1];
    expect(g.kind === "group" && [g.key, g.steps, g.running, g.ms]).toEqual(["group-t1", 3, false, 72_000]);
    expect(formatDuration(72_000)).toBe("1m 12s");
  });

  it("leaves shorter runs alone and breaks runs at text", () => {
    expect(shape([tool("t1", 0), tool("t2", 0), text("a"), tool("t3", 0), tool("t4", 0)])).toEqual([
      "t1",
      "t2",
      "a",
      "t3",
      "t4",
    ]);
    expect(shape([tool("t1", 0), tool("t2", 0), tool("t3", 0), text("a"), tool("t4", 0)])).toEqual([
      "[t1 t2 t3]",
      "a",
      "t4",
    ]);
  });

  it("keeps thinking inside a run and marks a run with an open call as running", () => {
    const think: ChatRow = { kind: "thinking", key: "th", text: "hmm", ts: 0 };
    const items = groupRows([tool("t1", 0), think, tool("t2", 5000), tool("t3", 9000, null)]);
    expect(items).toHaveLength(1);
    const g = items[0];
    expect(g.kind === "group" && [g.rows.length, g.steps, g.running, g.ms]).toEqual([4, 3, true, 9000]);
  });

  it("finds the latest call of a tool", () => {
    const todo = (key: string): ChatRow => ({ ...(tool(key, 0) as object), name: "TodoWrite" }) as ChatRow;
    expect(lastToolKey([todo("a"), tool("b", 0), todo("c"), text("d")], "TodoWrite")).toBe("c");
    expect(lastToolKey([text("d")], "TodoWrite")).toBeUndefined();
  });
});

describe("formatDuration", () => {
  it("reads in seconds, minutes and hours", () => {
    expect(formatDuration(4_400)).toBe("4s");
    expect(formatDuration(3_725_000)).toBe("1h 2m");
  });
});

describe("toolKind", () => {
  it("groups tools by icon family", () => {
    expect(toolKind("Bash")).toBe("shell");
    expect(toolKind("MultiEdit")).toBe("file");
    expect(toolKind("Glob")).toBe("search");
    expect(toolKind("Task")).toBe("agent");
    expect(toolKind("WebFetch")).toBe("web");
    expect(toolKind("mcp__x__y")).toBe("other");
  });
});

describe("turns", () => {
  it("folds Claude's items between prompts into one turn", () => {
    const rows = chatRows(
      [
        entry("u", { role: "user", text: "Fix it", ts: 1 }),
        entry("t", { kind: "thinking", text: "hmm", ts: 2 }),
        entry("a", { text: "Looking.", ts: 4_002, model: "m" }),
        entry("c", { kind: "tool_call", text: "", tool: { name: "Read", summary: "a" }, ts: 5_000 }),
        entry("a2", { text: "Fixed.", ts: 6_000 }),
        entry("u2", { role: "user", text: "Thanks", ts: 7_000 }),
      ],
      { thinking: true },
    );
    // The thinking lasts until the next entry.
    expect(rows[1].kind === "thinking" && rows[1].ms).toBe(4_000);
    const turns = turnsOf(groupRows(rows));
    expect(turns.map((t) => [t.kind, t.key, t.items.length, t.text, t.model])).toEqual([
      ["user", "u", 1, "Fix it", undefined],
      ["assistant", "t", 4, "Looking.\n\nFixed.", "m"],
      ["user", "u2", 1, "Thanks", undefined],
    ]);
    // A result that lands changes the turn's signature.
    const done = turnsOf(groupRows(chatRows([...rowsEntries(), entry("r", { role: "user", kind: "tool_result", text: "ok" })])));
    expect(done[0].sig).not.toBe(turnsOf(groupRows(chatRows(rowsEntries())))[0].sig);
  });

  function rowsEntries() {
    return [entry("c", { kind: "tool_call", text: "", tool: { name: "Bash", summary: "ls" } })];
  }

  it("labels days relative to now", () => {
    const now = new Date(2026, 9, 20, 12).getTime();
    expect(dayLabel(new Date(2026, 9, 20, 1).getTime(), now)).toBe("Today");
    expect(dayLabel(new Date(2026, 9, 19, 23).getTime(), now)).toBe("Yesterday");
    expect(dayLabel(new Date(2026, 9, 12).getTime(), now)).toBe("Mon, Oct 12");
    expect(dayLabel(new Date(2025, 9, 12).getTime(), now)).toBe("Sun, Oct 12, 2025");
  });

  it("strips markdown for plain text and quotes for the composer", () => {
    expect(plainText("## Done\nRan **cargo test** and `npm test`, see [the PR](https://x).")).toBe(
      "Done\nRan cargo test and npm test, see the PR.",
    );
    expect(quoted("a\n\nb")).toBe("> a\n>\n> b\n\n");
  });


});
