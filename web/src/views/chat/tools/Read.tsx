import { FileSearchIcon } from "lucide-react";
import { CodeBlock, CodeBlockContent, CodeBlockCopyButton, CodeBlockLine } from "@/components/ui/code-block";
import { langOf, useLines } from "./code";
import { Generic } from "./Generic";
import { CLAMP, FullOutputButton, num, PathChip, str, ToolCard, useFullResult, type ToolProps } from "./parts";

const NUMBERED = /^\s*(\d+)[→\t](.*)$/;

/**
 * Read output as `cat -n` gives it (`   12→text`): the first line number and
 * the bare text, or `null` when the output has another shape.
 */
export function unnumber(out: string): { start: number; text: string } | null {
  const lines = out.replace(/\n$/, "").split("\n");
  const m = lines.map((l) => NUMBERED.exec(l));
  if (m.length === 0 || m.some((x) => !x)) return null;
  return { start: Number(m[0]![1]), text: m.map((x) => x![2]).join("\n") };
}

function FileOutput(p: ToolProps & { path: string }) {
  const full = useFullResult(p);
  const bare = unnumber(full.text);
  const lines = useLines(bare?.text ?? full.text, bare ? langOf(p.path) : "text");
  return (
    <>
      <CodeBlock size="sm" variant={p.row.isError ? "destructive" : "default"}>
        <CodeBlockCopyButton value={bare?.text ?? full.text} className="absolute end-1 top-1 z-10 bg-background" />
        <CodeBlockContent lineNumbers={!!bare} startLine={bare?.start} wrap={!bare} className={CLAMP}>
          {lines.map((l, i) => (
            <CodeBlockLine key={i}>{l}</CodeBlockLine>
          ))}
        </CodeBlockContent>
      </CodeBlock>
      <FullOutputButton full={full} total={p.row.resultLen} />
    </>
  );
}

/** Read: the file and the line range; the content only when expanded. */
export function Read(p: ToolProps) {
  const path = str(p.row.input, "file_path");
  if (!path) return <Generic {...p} />;
  const offset = num(p.row.input, "offset");
  const limit = num(p.row.input, "limit");
  const from = offset ?? 1;
  const range =
    offset !== undefined || limit !== undefined
      ? limit !== undefined
        ? `lines ${from}–${from + limit - 1}`
        : `from line ${from}`
      : undefined;
  return (
    <ToolCard row={p.row} icon={<FileSearchIcon />} open={p.open} title={<PathChip path={path} cwd={p.cwd} />} meta={range}>
      {p.row.result ? <FileOutput {...p} path={path} /> : null}
    </ToolCard>
  );
}
