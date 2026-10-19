import { describe, expect, it } from "vitest";
import { chatRows, KEEP, mergeEntries, toolKind } from "./chat";
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
      { kind: "user", key: "u", text: "Add pagination" },
      { kind: "assistant", key: "a", text: "On it." },
      { kind: "tool", key: "c1", name: "Bash", summary: "git push", result: "pushed", isError: false, resultKey: "r1" },
      { kind: "tool", key: "c2", name: "Read", summary: "src/a.rs", result: "fn a()", isError: false, resultKey: "r2" },
      { kind: "assistant", key: "a2", text: "Done." },
    ]);
  });

  it("shows a running call without a result, and a result without a call on its own", () => {
    const rows = chatRows([
      entry("r", { role: "user", kind: "tool_result", text: "orphan" }),
      entry("c", { kind: "tool_call", text: "x", tool: { name: "Bash", summary: "sleep 9" } }),
    ]);
    expect(rows).toEqual([
      { kind: "tool", key: "r", name: "tool", summary: "", result: "orphan", isError: false, resultKey: "r" },
      { kind: "tool", key: "c", name: "Bash", summary: "sleep 9", result: null, isError: false },
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
    expect(rows[0]).toEqual({ kind: "thinking", key: "t", text: "hmm" });
    expect(rows[1]).toEqual({ kind: "assistant", key: "a", text: "Edit it.", model: "claude-opus-5-5" });
    expect(rows[2].kind === "tool" && rows[2].input).toEqual(input);
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
