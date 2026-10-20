// A line diff for the Edit card: the longest common subsequence of the old
// and the new lines, as unified lines with their line numbers on each side.

export interface DiffLine {
  op: " " | "+" | "-";
  text: string;
  /** The line number in the old text (context and removed lines). */
  old?: number;
  /** The line number in the new text (context and added lines). */
  new?: number;
}

/** Above this many LCS cells the middle shows as one replace block. */
const MAX_CELLS = 4_000_000;

const lines = (s: string) => (s === "" ? [] : s.replace(/\n$/, "").split("\n"));

/** The unified diff of `a` to `b`, line by line. Identical inputs give only context lines. */
export function diffLines(a: string, b: string): DiffLine[] {
  const x = lines(a);
  const y = lines(b);
  // Common head and tail cost nothing; only the middle needs the table.
  let head = 0;
  while (head < x.length && head < y.length && x[head] === y[head]) head++;
  let tail = 0;
  while (tail < x.length - head && tail < y.length - head && x[x.length - 1 - tail] === y[y.length - 1 - tail]) tail++;
  const xm = x.slice(head, x.length - tail);
  const ym = y.slice(head, y.length - tail);

  const out: DiffLine[] = [];
  let o = 1;
  let n = 1;
  const ctx = (text: string) => out.push({ op: " ", text, old: o++, new: n++ });
  const del = (text: string) => out.push({ op: "-", text, old: o++ });
  const add = (text: string) => out.push({ op: "+", text, new: n++ });

  x.slice(0, head).forEach(ctx);
  if (xm.length * ym.length > MAX_CELLS) {
    xm.forEach(del);
    ym.forEach(add);
  } else {
    // len[i][j]: LCS length of xm[i..] and ym[j..].
    const w = ym.length + 1;
    const len = new Uint32Array((xm.length + 1) * w);
    for (let i = xm.length - 1; i >= 0; i--)
      for (let j = ym.length - 1; j >= 0; j--)
        len[i * w + j] = xm[i] === ym[j] ? len[(i + 1) * w + j + 1] + 1 : Math.max(len[(i + 1) * w + j], len[i * w + j + 1]);
    let i = 0;
    let j = 0;
    while (i < xm.length && j < ym.length) {
      if (xm[i] === ym[j]) {
        ctx(xm[i++]);
        j++;
      } else if (len[(i + 1) * w + j] >= len[i * w + j + 1]) del(xm[i++]);
      else add(ym[j++]);
    }
    while (i < xm.length) del(xm[i++]);
    while (j < ym.length) add(ym[j++]);
  }
  x.slice(x.length - tail).forEach(ctx);
  return out;
}

/** Added and removed line counts. */
export function diffStats(diff: DiffLine[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const l of diff) {
    if (l.op === "+") added++;
    else if (l.op === "-") removed++;
  }
  return { added, removed };
}

/** One row of the split view: the old line on the left, the new on the right. */
export interface SplitRow {
  left?: DiffLine;
  right?: DiffLine;
}

/** Pairs each block of removed lines with the added lines that follow it. */
export function splitRows(diff: DiffLine[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let i = 0;
  while (i < diff.length) {
    if (diff[i].op === " ") {
      rows.push({ left: diff[i], right: diff[i] });
      i++;
      continue;
    }
    const del: DiffLine[] = [];
    const add: DiffLine[] = [];
    while (i < diff.length && diff[i].op === "-") del.push(diff[i++]);
    while (i < diff.length && diff[i].op === "+") add.push(diff[i++]);
    for (let k = 0; k < Math.max(del.length, add.length); k++) rows.push({ left: del[k], right: add[k] });
  }
  return rows;
}
