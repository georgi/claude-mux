// The pieces every tool card shares: the props, the trace line with its
// status, the path chip and the output block with "Show full output".

import { useState, type ReactNode } from "react";
import {
  CircleCheckIcon,
  CircleXIcon,
  FileSearchIcon,
  FileTextIcon,
  GlobeIcon,
  LoaderCircleIcon,
  SearchIcon,
  TerminalIcon,
  UsersIcon,
  WrenchIcon,
} from "lucide-react";
import type { Api } from "@/api/http";
import type { AgentView, JsonValue } from "@/api/types";
import { toolKind, type ToolKind, type ToolRow } from "@/chat";
import { Button } from "@/components/ui/button";
import { CodeBlock, CodeBlockContent, CodeBlockCopyButton } from "@/components/ui/code-block";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import { cn } from "@/lib/utils";

/** What the chat log knows besides the row: whose chat it is and how to act. */
export interface ToolContext {
  /** The agent whose chat this is. */
  agent: string;
  api?: Api;
  /** Every agent, to find this agent's sub-agents. */
  agents?: AgentView[];
  /** The agent's working directory; paths under it show relative. */
  cwd?: string;
  onOpenAgent?: (agent: string) => void;
}

export interface ToolProps extends ToolContext {
  row: ToolRow;
  /** Expanded at first. */
  open?: boolean;
}

const ICONS: Record<ToolKind, React.ComponentType<{ className?: string }>> = {
  shell: TerminalIcon,
  file: FileTextIcon,
  read: FileSearchIcon,
  search: SearchIcon,
  agent: UsersIcon,
  web: GlobeIcon,
  other: WrenchIcon,
};

/** The icon of a tool's family. */
export function KindIcon({ name }: { name: string }) {
  const Icon = ICONS[toolKind(name)];
  return <Icon />;
}

/** Long output clamps; the block scrolls inside itself. */
export const CLAMP = "max-h-64";

/** The tool whose call starts a cmux sub-agent; its card is the Task card. */
export const SPAWN_SUBAGENT = "mcp__cmux__spawn_subagent";

/**
 * The row has more to draw than its summary: a structured input (a newer
 * server), or a tool whose card reads the summary. Rows without it show the
 * plain summary line and need no renderer code.
 */
export function hasCard(row: ToolRow): boolean {
  return (row.input !== undefined && row.input !== null) || row.name === SPAWN_SUBAGENT;
}

type Obj = { [key: string]: JsonValue };

export function obj(v: JsonValue | undefined): Obj | undefined {
  return v !== null && typeof v === "object" && !Array.isArray(v) ? v : undefined;
}

export function str(v: JsonValue | undefined, key: string): string | undefined {
  const x = obj(v)?.[key];
  return typeof x === "string" ? x : undefined;
}

export function num(v: JsonValue | undefined, key: string): number | undefined {
  const x = obj(v)?.[key];
  return typeof x === "number" ? x : undefined;
}

export function list(v: JsonValue | undefined, key: string): JsonValue[] {
  const x = obj(v)?.[key];
  return Array.isArray(x) ? x : [];
}

/** `path` relative to `cwd` when it lies under it. */
export function relPath(path: string, cwd?: string): string {
  if (cwd && path.startsWith(cwd.replace(/\/$/, "") + "/")) return path.slice(cwd.replace(/\/$/, "").length + 1);
  return path;
}

/** A file path in mono; a long one truncates in the middle and keeps its file name. */
export function PathChip({ path, cwd, className }: { path: string; cwd?: string; className?: string }) {
  const shown = relPath(path, cwd);
  const cut = shown.lastIndexOf("/") + 1;
  return (
    <span
      title={path}
      className={cn("inline-flex min-w-0 items-center rounded-sm bg-muted px-1 font-mono text-xs text-foreground", className)}
    >
      <span className="min-w-0 truncate">{shown.slice(0, cut)}</span>
      <span className="shrink-0">{shown.slice(cut)}</span>
    </span>
  );
}

/** The running, error or done mark at the end of a trace line. */
function StatusMark({ row }: { row: ToolRow }) {
  if (row.result === null) return null;
  return (
    <TraceDescription>
      {row.isError ? (
        <CircleXIcon className="size-3.5 text-status-failing" aria-label="Error" role="img" />
      ) : (
        <CircleCheckIcon className="size-3.5" aria-label="Done" role="img" />
      )}
    </TraceDescription>
  );
}

interface CardProps {
  row: ToolRow;
  icon: ReactNode;
  /** The line's text: what the call did. */
  title: ReactNode;
  /** Short facts after the title, such as counts. */
  meta?: ReactNode;
  /** Beside the line, outside the toggle: a button that acts. */
  action?: ReactNode;
  open?: boolean;
  /** The folded body; none makes the line plain. */
  children?: ReactNode;
}

/** One tool call: its trace line, and its body folded under it. */
export function ToolCard({ row, icon, title, meta, action, open, children }: CardProps) {
  const running = row.result === null;
  const truncated = obj(row.input)?.truncated === true;
  const line = (
    <TraceLine status={running ? "running" : row.isError ? "error" : undefined}>
      <TraceIcon>{running ? <LoaderCircleIcon className="animate-spin" /> : icon}</TraceIcon>
      <TraceTitle className="flex min-w-0 items-center gap-1.5">{title}{truncated && <span title="The server truncated this tool input; the preview is incomplete." className="shrink-0 text-xs text-muted-foreground">Partial input</span>}</TraceTitle>
      {meta && <TraceDescription className="flex items-center gap-1.5 text-xs">{meta}</TraceDescription>}
      <StatusMark row={row} />
    </TraceLine>
  );
  if (!children)
    return (
      <Trace>
        <div className="flex min-w-0 items-center gap-2">
          {line}
          {action}
        </div>
      </Trace>
    );
  return (
    <Trace>
      <TraceCollapsible defaultOpen={open}>
        <div className="flex min-w-0 items-center gap-2">
          <TraceCollapsibleTrigger aria-label={`${row.name} output`}>{line}</TraceCollapsibleTrigger>
          {action}
        </div>
        <TraceCollapsibleContent className="flex flex-col gap-2">{children}</TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}

/**
 * The result, uncut on demand: `load` fetches the entry when the server cut
 * it (`resultLen`). `more` says a fetch can help.
 */
export function useFullResult({ row, agent, api }: ToolProps) {
  const [full, setFull] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const more = full === null && !!api && !!row.resultKey && row.resultLen !== undefined;
  const load = async () => {
    if (!api || !row.resultKey) return;
    setBusy(true);
    setError(null);
    try {
      setFull((await api.entry(agent, row.resultKey)).text);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return { text: full ?? row.result ?? "", more, busy, error, load };
}

export function FullOutputButton({ full, total }: { full: ReturnType<typeof useFullResult>; total?: number }) {
  if (!full.more && !full.error) return null;
  return (
    <div className="flex items-center gap-2 text-xs">
      {full.more && (
        <Button size="xs" variant="outline" disabled={full.busy} onClick={() => void full.load()}>
          {full.busy ? <LoaderCircleIcon className="animate-spin" /> : null}
          Show full output{total !== undefined ? ` (${total.toLocaleString("en-US")} chars)` : ""}
        </Button>
      )}
      {full.error && (
        <span role="alert" className="text-destructive">
          {full.error}
        </span>
      )}
    </div>
  );
}

/** Raw output in a code block, with a copy button. */
export function Output({ text, wrap = true, className }: { text: string; wrap?: boolean; className?: string }) {
  return (
    <CodeBlock size="sm" className={className}>
      <CodeBlockCopyButton value={text} className="absolute end-1 top-1 z-10 bg-background" />
      <CodeBlockContent wrap={wrap} className={CLAMP}>
        {text}
      </CodeBlockContent>
    </CodeBlock>
  );
}

/** The result with "Show full output" under it; nothing while the call runs. */
export function ResultOutput(p: ToolProps & { wrap?: boolean }) {
  const full = useFullResult(p);
  if (p.row.result === null || full.text === "") return null;
  return (
    <>
      <Output text={full.text} wrap={p.wrap} className={p.row.isError ? "border-destructive/40" : undefined} />
      <FullOutputButton full={full} total={p.row.resultLen} />
    </>
  );
}
