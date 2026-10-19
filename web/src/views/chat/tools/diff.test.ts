import { describe, expect, it } from "vitest";
import { diffLines, diffStats, splitRows } from "./diff";

const ops = (a: string, b: string) => diffLines(a, b).map((l) => `${l.op}${l.text}`);

describe("diffLines", () => {
  it("gives only context lines for identical inputs", () => {
    const d = diffLines("a\nb\n", "a\nb\n");
    expect(d.map((l) => `${l.op}${l.text}`)).toEqual([" a", " b"]);
    expect(diffStats(d)).toEqual({ added: 0, removed: 0 });
    expect(diffLines("", "")).toEqual([]);
  });

  it("finds inserted lines", () => {
    expect(ops("a\nc", "a\nb\nc")).toEqual([" a", "+b", " c"]);
    expect(ops("", "x\ny")).toEqual(["+x", "+y"]);
  });

  it("finds deleted lines", () => {
    expect(ops("a\nb\nc", "a\nc")).toEqual([" a", "-b", " c"]);
    expect(ops("x", "")).toEqual(["-x"]);
  });

  it("shows a replaced line as removed then added, with line numbers", () => {
    const d = diffLines("fn a() {\n  1\n}", "fn a() {\n  2\n  3\n}");
    expect(d.map((l) => `${l.op}${l.text}`)).toEqual([" fn a() {", "-  1", "+  2", "+  3", " }"]);
    expect(diffStats(d)).toEqual({ added: 2, removed: 1 });
    expect(d.map((l) => [l.old, l.new])).toEqual([
      [1, 1],
      [2, undefined],
      [undefined, 2],
      [undefined, 3],
      [3, 4],
    ]);
  });

  it("keeps the longest common lines in the middle", () => {
    expect(ops("a\nx\nb\ny\nc", "a\nb\nz\nc")).toEqual([" a", "-x", " b", "-y", "+z", " c"]);
  });
});

describe("splitRows", () => {
  it("pairs removed with added lines side by side", () => {
    const rows = splitRows(diffLines("a\nb\nc", "a\nB\nB2\nc"));
    expect(rows.map((r) => [r.left?.text ?? null, r.right?.text ?? null])).toEqual([
      ["a", "a"],
      ["b", "B"],
      [null, "B2"],
      ["c", "c"],
    ]);
  });
});
