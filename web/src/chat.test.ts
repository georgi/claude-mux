import { describe, expect, it } from "vitest";
import { chatRows, KEEP, mergeEntries } from "./chat";
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
      { kind: "tool", key: "c1", name: "Bash", summary: "git push", result: "pushed" },
      { kind: "tool", key: "c2", name: "Read", summary: "src/a.rs", result: "fn a()" },
      { kind: "assistant", key: "a2", text: "Done." },
    ]);
  });

  it("shows a running call without a result, and a result without a call on its own", () => {
    const rows = chatRows([
      entry("r", { role: "user", kind: "tool_result", text: "orphan" }),
      entry("c", { kind: "tool_call", text: "x", tool: { name: "Bash", summary: "sleep 9" } }),
    ]);
    expect(rows).toEqual([
      { kind: "tool", key: "r", name: "tool", summary: "", result: "orphan" },
      { kind: "tool", key: "c", name: "Bash", summary: "sleep 9", result: null },
    ]);
  });
});
