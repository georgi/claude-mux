import { useMemo, useState } from "react";
import { FilePenIcon } from "lucide-react";
import { useMedia } from "@/hooks";
import { CodeBlock, CodeBlockContent } from "@/components/ui/code-block";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { cn } from "@/lib/utils";
import { langOf, useLines } from "./code";
import { diffLines, diffStats, splitRows, type DiffLine } from "./diff";
import { Generic } from "./Generic";
import { CLAMP, list, obj, PathChip, ResultOutput, str, ToolCard, type ToolProps } from "./parts";

type Mode = "unified" | "split";
const MODE_KEY = "cmux.diff.mode";

function savedMode(): Mode {
  try {
    return localStorage.getItem(MODE_KEY) === "split" ? "split" : "unified";
  } catch {
    return "unified";
  }
}

interface Hunk {
  old: string;
  new: string;
  lines: DiffLine[];
}

/** The `+N −M` counts; also the screen-reader text. */
export function DiffCounts({ added, removed }: { added: number; removed: number }) {
  return (
    <span className="font-mono tabular-nums" aria-label={`${added} added, ${removed} removed`}>
      <span className="text-status-passing">+{added}</span> <span className="text-status-failing">−{removed}</span>
    </span>
  );
}

const ROW: Record<DiffLine["op"], string> = {
  " ": "",
  "+": "bg-status-passing/10",
  "-": "bg-status-failing/10",
};
const MARK: Record<DiffLine["op"], string> = { " ": " ", "+": "+", "-": "−" };

function Gutter({ n }: { n?: number }) {
  return <span className="w-8 shrink-0 pr-2 text-right text-muted-foreground/60 select-none">{n ?? ""}</span>;
}

/** One line: its number, its mark and its highlighted content. */
function Line({ line, node, nums }: { line?: DiffLine; node: React.ReactNode; nums: (number | undefined)[] }) {
  return (
    <span data-op={line?.op} className={cn("flex min-h-lh", line ? ROW[line.op] : "bg-muted/40")}>
      {nums.map((n, i) => (
        <Gutter key={i} n={n} />
      ))}
      <span
        aria-hidden="true"
        className={cn(
          "w-4 shrink-0 select-none",
          line?.op === "+" && "text-status-passing",
          line?.op === "-" && "text-status-failing",
        )}
      >
        {line ? MARK[line.op] : ""}
      </span>
      <span className="pr-3 whitespace-pre">{node}</span>
    </span>
  );
}

/** One edit as a unified or a split diff, highlighted by the file's language. */
function HunkView({ hunk, lang, mode }: { hunk: Hunk; lang: string; mode: Mode }) {
  const oldLines = useLines(hunk.old, lang);
  const newLines = useLines(hunk.new, lang);
  const node = (l?: DiffLine) => (!l ? null : l.op === "+" ? newLines[l.new! - 1] : oldLines[l.old! - 1]) ?? l?.text;
  if (mode === "split") {
    return (
      <div className="grid grid-cols-2 divide-x">
        {(["left", "right"] as const).map((side) => (
          <CodeBlockContent key={side} className={cn(CLAMP, "px-0 py-1")}>
            {splitRows(hunk.lines).map((r, i) => {
              const l = r[side];
              return <Line key={i} line={l} node={node(l)} nums={[side === "left" ? l?.old : l?.new]} />;
            })}
          </CodeBlockContent>
        ))}
      </div>
    );
  }
  return (
    <CodeBlockContent className={cn(CLAMP, "px-0 py-1")}>
      {hunk.lines.map((l, i) => (
        <Line key={i} line={l} node={node(l)} nums={[l.old, l.new]} />
      ))}
    </CodeBlockContent>
  );
}

/** Edit and MultiEdit: the file, `+N −M`, and the diff of each edit. */
export function Edit(p: ToolProps) {
  const path = str(p.row.input, "file_path");
  const pairs = useMemo(() => {
    const input = p.row.input;
    const edits = p.row.name === "MultiEdit" ? list(input, "edits").map(obj) : [obj(input)];
    return edits
      .filter((e) => e !== undefined)
      .map((e) => ({ old: str(e, "old_string") ?? "", new: str(e, "new_string") ?? "" }))
      .map((e) => ({ ...e, lines: diffLines(e.old, e.new) }));
  }, [p.row.input, p.row.name]);
  const wide = useMedia("(min-width: 1024px)");
  const [mode, setMode] = useState<Mode>(savedMode);
  if (!path || pairs.length === 0) return <Generic {...p} />;

  const stats = pairs.map((h) => diffStats(h.lines));
  const added = stats.reduce((a, s) => a + s.added, 0);
  const removed = stats.reduce((a, s) => a + s.removed, 0);
  const shown: Mode = wide ? mode : "unified";
  const lang = langOf(path);
  const pick = (v: unknown[]) => {
    const m = v[0] === "split" ? "split" : v[0] === "unified" ? "unified" : null;
    if (!m) return;
    setMode(m);
    try {
      localStorage.setItem(MODE_KEY, m);
    } catch {
      // Private mode: the choice lasts until reload.
    }
  };

  return (
    <ToolCard
      row={p.row}
      icon={<FilePenIcon />}
      open={p.open}
      title={<PathChip path={path} cwd={p.cwd} />}
      meta={obj(p.row.input)?.truncated === true ? undefined : <DiffCounts added={added} removed={removed} />}
    >
      {wide && (
        <ToggleGroup size="sm" variant="outline" spacing={0} value={[shown]} onValueChange={pick} aria-label="Diff view">
          <ToggleGroupItem value="unified" className="h-6 px-2 text-xs">
            Unified
          </ToggleGroupItem>
          <ToggleGroupItem value="split" className="h-6 px-2 text-xs">
            Split
          </ToggleGroupItem>
        </ToggleGroup>
      )}
      <CodeBlock size="sm" aria-label={`Changes to ${path}`}>
        {pairs.map((h, i) => (
          <div key={i} className={cn(i > 0 && "border-t border-dashed")}>
            <HunkView hunk={h} lang={lang} mode={shown} />
          </div>
        ))}
      </CodeBlock>
      {p.row.isError && <ResultOutput {...p} />}
    </ToolCard>
  );
}
