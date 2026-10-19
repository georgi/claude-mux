import { createContext, lazy, memo, Suspense, useContext, useEffect, useRef, useState } from "react";
import { BrainIcon, ChevronRightIcon, CopyIcon, FileTextIcon, QuoteIcon } from "lucide-react";
import { toast } from "sonner";
import { formatDuration, plainText, quoted, type ChatRow, type LogItem, type ToolRow as Row, type Turn } from "@/chat";
import { relativeTime } from "@/lib/time";
import { cn } from "@/lib/utils";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Message, MessageContent } from "@/components/ui/message";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Markdown } from "../lazy";
import { quoteInto } from "./Composer";
import { Generic } from "./tools/Generic";
import { ToolGroupView } from "./tools/Group";
import { hasCard, type ToolContext } from "./tools/parts";

// The tool cards load with the first row that has a structured input. Until
// then, and for rows from an older server, the summary line shows.
const ToolView = lazy(() => import("./tools").then((m) => ({ default: m.ToolView })));

/** The current time for every time label in the log, so one timer serves them all. */
export const NowContext = createContext(Date.now());

/**
 * Tells a turn when it comes near the log's viewport. The log provides one
 * observer for all its turns; without one (tests, old browsers) every turn
 * is near.
 */
export type Watch = (el: Element, near: () => void) => () => void;
export const WatchContext = createContext<Watch | null>(null);

/** One IntersectionObserver on `root` that fires each element's callback once it is within `margin`. */
export function makeWatch(root: () => Element | null, margin = "1200px 0px"): Watch | null {
  if (typeof IntersectionObserver === "undefined") return null;
  const calls = new Map<Element, () => void>();
  let io: IntersectionObserver | undefined;
  return (el, near) => {
    io ??= new IntersectionObserver(
      (es) => {
        for (const e of es) if (e.isIntersecting) calls.get(e.target)?.();
      },
      { root: root(), rootMargin: margin },
    );
    calls.set(el, near);
    io.observe(el);
    return () => {
      calls.delete(el);
      io?.unobserve(el);
    };
  };
}

/** True once the element has come near the viewport; it stays true. */
function useNear(ref: React.RefObject<Element | null>, initial: boolean): boolean {
  const watch = useContext(WatchContext);
  const [near, setNear] = useState(initial || watch === null);
  useEffect(() => {
    const el = ref.current;
    if (near || !el || !watch) return;
    return watch(el, () => setNear(true));
  }, [near, watch, ref]);
  return near || watch === null;
}

const absolute = new Intl.DateTimeFormat("en-US", { dateStyle: "medium", timeStyle: "medium" });

const hoverOnly =
  "opacity-0 transition-opacity group-hover/turn:opacity-100 group-focus-within/turn:opacity-100 pointer-coarse:opacity-100";

/** "2m", with the absolute time in a tooltip. It shows on hover or focus of its turn. */
function TimeLabel({ ts }: { ts: number }) {
  const now = useContext(NowContext);
  const full = absolute.format(ts);
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <time
            dateTime={new Date(ts).toISOString()}
            aria-label={full}
            className={cn("text-xs text-muted-foreground tabular-nums", hoverOnly)}
          />
        }
      >
        {relativeTime(ts, now)}
      </TooltipTrigger>
      <TooltipContent>{full}</TooltipContent>
    </Tooltip>
  );
}

/** Copies `text` and says so. */
export async function copyText(text: string, what: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    toast(`Copied ${what}`);
  } catch {
    toast.error("Could not copy. The browser blocked the clipboard.");
  }
}

/** The three things you can do with a message, shared by the hover toolbar and the long-press menu. */
export const MESSAGE_ACTIONS = [
  { id: "text", label: "Copy text", icon: CopyIcon, run: (t: string) => void copyText(plainText(t), "text") },
  { id: "markdown", label: "Copy as Markdown", icon: FileTextIcon, run: (t: string) => void copyText(t, "Markdown") },
  { id: "quote", label: "Quote in reply", icon: QuoteIcon, run: (t: string, agent: string) => quoteInto(agent, quoted(t)) },
] as const;

function Actions({ text, agent }: { text: string; agent: string }) {
  return (
    <div role="toolbar" aria-label="Message actions" className={cn("flex items-center gap-0.5 pointer-coarse:hidden", hoverOnly)}>
      {MESSAGE_ACTIONS.map((a) => (
        <Tooltip key={a.id}>
          <TooltipTrigger
            render={<Button variant="ghost" size="icon-xs" aria-label={a.label} onClick={() => a.run(text, agent)} />}
          >
            <a.icon />
          </TooltipTrigger>
          <TooltipContent>{a.label}</TooltipContent>
        </Tooltip>
      ))}
    </div>
  );
}

/** The Reasoning pattern: "Thought for 4s", folded; the thinking itself under it. */
export function Thinking({ row, live }: { row: Extract<ChatRow, { kind: "thinking" }>; live: boolean }) {
  const thinking = row.ms === undefined && live;
  const label = thinking ? "Thinking…" : thoughtLabel(row);
  return (
    <Collapsible className="flex min-w-0 flex-col">
      <CollapsibleTrigger className="group/reasoning flex w-fit items-center gap-2 rounded-md text-sm text-muted-foreground transition-colors outline-none hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50">
        <BrainIcon className="size-4" aria-hidden="true" />
        <span className={thinking ? "shimmer motion-reduce:shimmer-none" : undefined}>{label}</span>
        <ChevronRightIcon className="size-4 transition-transform group-aria-expanded/reasoning:rotate-90" aria-hidden="true" />
      </CollapsibleTrigger>
      <CollapsibleContent className="h-(--collapsible-panel-height) min-w-0 overflow-hidden transition-[height] duration-200 data-ending-style:h-0 data-starting-style:h-0">
        <p className="mt-2 ml-[calc(--spacing(2)-1px)] border-l pl-4 text-sm whitespace-pre-wrap text-muted-foreground">{row.text}</p>
      </CollapsibleContent>
    </Collapsible>
  );
}

export function ToolRow({ row, tools, open }: { row: Row; tools?: ToolContext; open?: boolean }) {
  const p = { agent: "", ...tools, row, open };
  if (!hasCard(row)) return <Generic {...p} />;
  return (
    <Suspense fallback={<Generic {...p} />}>
      <ToolView {...p} />
    </Suspense>
  );
}

interface TurnProps {
  turn: Turn;
  tools?: ToolContext;
  /** The key of the one tool row that opens by itself (the latest todo list), when it is in this turn. */
  open?: string;
  /** The latest turn while the agent works: its reply streams, its thinking shimmers. */
  live: boolean;
  /** Render the full turn at once, without waiting for the observer (the turns at the bottom). */
  near: boolean;
}

function thoughtLabel(row: Extract<ChatRow, { kind: "thinking" }>): string {
  return row.ms === undefined ? "Thought" : `Thought for ${formatDuration(Math.max(1000, row.ms))}`;
}

function Item({ item, tools, open, live }: { item: LogItem; tools?: ToolContext; open?: string; live: boolean }) {
  switch (item.kind) {
    case "group":
      return (
        <ToolGroupView group={item}>
          {item.rows.map((t) =>
            t.kind === "tool" ? (
              <ToolRow key={t.key} row={t} tools={tools} open={t.key === open} />
            ) : t.kind === "thinking" ? (
              <Thinking key={t.key} row={t} live={live} />
            ) : null,
          )}
        </ToolGroupView>
      );
    case "tool":
      return <ToolRow row={item} tools={tools} open={item.key === open} />;
    case "thinking":
      return <Thinking row={item} live={live} />;
    case "assistant":
      return <Markdown streaming={live}>{item.text}</Markdown>;
    default:
      return null;
  }
}

/**
 * One turn: a prompt as a bubble, or everything Claude did until the next
 * prompt under one header. Rows are rebuilt with each batch, so it renders
 * again only when its signature changes.
 */
function TurnViewImpl({ turn, tools, open, live, near: initial }: TurnProps) {
  const ref = useRef<HTMLDivElement>(null);
  const rich = useNear(ref, initial);
  const agent = tools?.agent ?? "";
  // A far turn is skipped by layout (content-visibility) and off screen; its
  // text stands in, for the size estimate and for find-in-page, until it
  // comes near. The cards, collapsibles and Markdown are the costly part.
  if (!rich)
    return (
      <div
        ref={ref}
        data-turn={turn.key}
        className={cn("text-sm whitespace-pre-wrap wrap-break-word", turn.kind === "user" && "ms-auto max-w-[80%]")}
      >
        {turn.text}
      </div>
    );
  if (turn.kind === "user")
    return (
      <Message ref={ref} align="end" data-turn={turn.key} className="group/turn">
        <MessageContent>
          <Bubble align="end" variant="muted">
            <BubbleContent className="whitespace-pre-wrap wrap-break-word">{turn.text}</BubbleContent>
          </Bubble>
          <div className="-mt-1 flex h-5 items-center justify-end gap-2">
            <Actions text={turn.text} agent={agent} />
            <TimeLabel ts={turn.ts} />
          </div>
        </MessageContent>
      </Message>
    );
  return (
    <Message ref={ref} data-turn={turn.key} className="group/turn">
      <MessageContent className="gap-3">
        <header className="flex h-6 items-center gap-2 text-xs">
          <span className="font-medium text-foreground">Claude</span>
          {turn.model && <span className="truncate font-mono text-muted-foreground">{turn.model.replace(/^claude-/, "")}</span>}
          <TimeLabel ts={turn.ts} />
          {turn.text && (
            <div className="ml-auto">
              <Actions text={turn.text} agent={agent} />
            </div>
          )}
        </header>
        {turn.items.map((i, n) => (
          <Item key={i.key} item={i} tools={tools} open={open} live={live && n === turn.items.length - 1} />
        ))}
      </MessageContent>
    </Message>
  );
}

export const TurnView = memo(
  TurnViewImpl,
  (a, b) => a.turn.sig === b.turn.sig && a.tools === b.tools && a.open === b.open && a.live === b.live,
);
