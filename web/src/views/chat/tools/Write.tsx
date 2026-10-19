import { FilePlusIcon } from "lucide-react";
import { CodeBlock, CodeBlockContent, CodeBlockCopyButton, CodeBlockLine } from "@/components/ui/code-block";
import { langOf, useLines } from "./code";
import { Generic } from "./Generic";
import { CLAMP, PathChip, ResultOutput, str, ToolCard, type ToolProps } from "./parts";

function Content({ path, content }: { path: string; content: string }) {
  const lines = useLines(content, langOf(path));
  return (
    <CodeBlock size="sm">
      <CodeBlockCopyButton value={content} className="absolute end-1 top-1 z-10 bg-background" />
      <CodeBlockContent lineNumbers className={CLAMP}>
        {lines.map((l, i) => (
          <CodeBlockLine key={i}>{l}</CodeBlockLine>
        ))}
      </CodeBlockContent>
    </CodeBlock>
  );
}

/** Write: the file, its line count, and its content folded and highlighted. */
export function Write(p: ToolProps) {
  const path = str(p.row.input, "file_path");
  const content = str(p.row.input, "content");
  if (!path || content === undefined) return <Generic {...p} />;
  const n = content === "" ? 0 : content.replace(/\n$/, "").split("\n").length;
  return (
    <ToolCard
      row={p.row}
      icon={<FilePlusIcon />}
      open={p.open}
      title={<PathChip path={path} cwd={p.cwd} />}
      meta={`${n} ${n === 1 ? "line" : "lines"}`}
    >
      <Content path={path} content={content} />
      {p.row.isError && <ResultOutput {...p} />}
    </ToolCard>
  );
}
