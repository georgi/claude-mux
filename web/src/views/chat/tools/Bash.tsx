import { CircleXIcon, TerminalIcon } from "lucide-react";
import { CodeBlock, CodeBlockContent, CodeBlockCopyButton, CodeBlockHeader, CodeBlockTitle } from "@/components/ui/code-block";
import { cn } from "@/lib/utils";
import { useLines } from "./code";
import { Generic } from "./Generic";
import { CLAMP, FullOutputButton, str, ToolCard, useFullResult, type ToolProps } from "./parts";

// eslint-disable-next-line no-control-regex
const ANSI = /\x1b\[[0-9;?]*[ -/]*[@-~]/g;

/** `text` without its ANSI escape sequences. */
export function stripAnsi(text: string): string {
  return text.replace(ANSI, "");
}

/** A stack frame line: `at f (file:1:2)`, `File "x.py", line 3`, `  src/a.rs:12:5`. */
const FRAME = /^\s+(at\s|File\s"|\S+\.\w+:\d+)|^\s*\d+:\s+0x[0-9a-f]+/;

/** Output lines, with ANSI colors rendered when the output has any. */
function TermLines({ text, error }: { text: string; error: boolean }) {
  const colored = text.includes("\x1b[");
  const lines = useLines(colored ? text : "", "ansi");
  if (colored) return <>{lines.map((l, i) => <span key={i} className="block min-h-lh">{l}</span>)}</>;
  // The Stack Trace pattern: the first line is the error, frames recede.
  let first = error;
  return (
    <>
      {text.replace(/\n$/, "").split("\n").map((l, i) => {
        const head = first && l.trim() !== "";
        if (head) first = false;
        return (
          <span
            key={i}
            className={cn(
              "block min-h-lh",
              head && "font-medium text-destructive",
              error && FRAME.test(l) && "text-muted-foreground",
            )}
          >
            {l}
          </span>
        );
      })}
    </>
  );
}

/** The Terminal pattern: the command, then its output; red with an error. */
function Terminal(p: ToolProps & { command: string }) {
  const full = useFullResult(p);
  const error = p.row.isError;
  return (
    <>
      <CodeBlock size="sm" variant={error ? "destructive" : "default"} role={error ? "alert" : undefined}>
        <CodeBlockHeader>
          <CodeBlockTitle className="font-mono">
            {error ? <CircleXIcon /> : <TerminalIcon />}
            {error ? "Failed: " : "$ "}
            {p.command.split("\n")[0]}
          </CodeBlockTitle>
        </CodeBlockHeader>
        <CodeBlockCopyButton value={stripAnsi(full.text)} className="absolute end-1 top-1 z-10 bg-background" />
        <CodeBlockContent className={cn(CLAMP, "text-foreground")}>
          {full.text ? <TermLines text={full.text} error={error} /> : <span className="text-muted-foreground">(no output)</span>}
        </CodeBlockContent>
      </CodeBlock>
      <FullOutputButton full={full} total={p.row.resultLen} />
    </>
  );
}

/** `$ command`, titled by its description; the output in a terminal block. */
export function Bash(p: ToolProps) {
  const command = str(p.row.input, "command");
  if (!command) return <Generic {...p} />;
  const description = str(p.row.input, "description");
  return (
    <ToolCard
      row={p.row}
      icon={<TerminalIcon />}
      open={p.open}
      title={
        <>
          {description && <span className="max-w-[60%] shrink-0 truncate font-medium">{description}</span>}
          <span className="min-w-0 truncate font-mono text-xs">$ {command.split("\n")[0]}</span>
        </>
      }
    >
      {command.includes("\n") && (
        <CodeBlock size="sm">
          <CodeBlockHeader>
            <CodeBlockTitle>Command</CodeBlockTitle>
          </CodeBlockHeader>
          <CodeBlockCopyButton value={command} aria-label="Copy command" className="absolute end-1 top-1 z-10 bg-background" />
          <CodeBlockContent wrap className={CLAMP} aria-label="Bash command">
            {command}
          </CodeBlockContent>
        </CodeBlock>
      )}
      {p.row.result !== null && <Terminal {...p} command={command} />}
    </ToolCard>
  );
}
