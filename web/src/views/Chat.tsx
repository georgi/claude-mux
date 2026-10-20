import { Fragment, useLayoutEffect, useMemo, useRef, useState } from "react";
import { MessageSquareIcon } from "lucide-react";
import type { Activity, ChatEntry } from "@/api/types";
import { chatRows, dayKey, dayLabel, groupRows, lastToolKey, turnsOf, type Turn } from "@/chat";
import { useNow } from "@/lib/time";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from "@/components/ui/context-menu";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/components/ui/message-scroller";
import { Skeleton } from "@/components/ui/skeleton";
import { MESSAGE_ACTIONS, makeWatch, NowContext, TurnView, WatchContext } from "./chat/Turn";
import { WorkingRow } from "./chat/Working";
import type { ToolContext } from "./chat/tools/parts";

export { Composer, queuedMessages, type QueuedMessage } from "./chat/Composer";

interface ChatProps {
  /** `undefined` while the first batch is on its way. */
  entries: ChatEntry[] | undefined;
  /** Whose chat this is, for the tool cards' actions (full output, open a sub-agent). */
  tools?: ToolContext;
  /** What the agent does now, for the live row under the log. No row without it. */
  activity?: Activity;
  /** When the agent went idle, in ms. */
  idleSince?: number | null;
  /** Rendered as the last item, for example a pending permission request. */
  children?: React.ReactNode;
}

/** The turns at the bottom render rich at once; the log opens there. */
const NEAR_END = 8;

function ChatSkeleton() {
  return (
    <div className="flex flex-1 flex-col gap-4 p-4" role="status" aria-label="Loading the conversation">
      <Skeleton className="ml-auto h-9 w-2/3" />
      <Skeleton className="h-4 w-5/6" />
      <Skeleton className="h-4 w-3/4" />
      <Skeleton className="h-5 w-1/2" />
    </div>
  );
}

function DaySeparator({ ts }: { ts: number }) {
  const now = useNow(60_000);
  return (
    <div role="separator" aria-label={dayLabel(ts, now)} className="flex items-center gap-3 text-xs text-muted-foreground">
      <span className="h-px flex-1 bg-border" />
      <span aria-hidden="true">{dayLabel(ts, now)}</span>
      <span className="h-px flex-1 bg-border" />
    </div>
  );
}

/** Where each agent's log was scrolled to, when not at the end. */
const scrolled = new Map<string, number>();

function useScrollMemory(key: string, viewport: React.RefObject<HTMLDivElement | null>, mounted: boolean) {
  useLayoutEffect(() => {
    const el = viewport.current;
    if (!el || !mounted) return;
    const saved = scrolled.get(key);
    let frame = 0;
    // The scroller puts the log at its end as content settles (lazy Markdown,
    // tool cards). Hold the saved place for a moment, until the user acts.
    const until = performance.now() + 600;
    const stop = () => cancelAnimationFrame(frame);
    const hold = () => {
      el.scrollTop = saved!;
      if (performance.now() < until) frame = requestAnimationFrame(hold);
    };
    if (saved !== undefined) frame = requestAnimationFrame(hold);
    const input = ["wheel", "touchstart", "pointerdown", "keydown"] as const;
    for (const t of input) el.addEventListener(t, stop, { passive: true });
    const onScroll = () => {
      if (el.scrollHeight - el.scrollTop - el.clientHeight < 48) scrolled.delete(key);
      else scrolled.set(key, el.scrollTop);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      stop();
      for (const t of input) el.removeEventListener(t, stop);
      el.removeEventListener("scroll", onScroll);
    };
  }, [key, viewport, mounted]);
}

/** Rough height of a turn before it renders, so the scrollbar is close to right. */
function intrinsic(t: Turn): string {
  const lines = t.kind === "user" ? 2 : Math.min(30, 2 + t.items.length * 2 + Math.ceil(t.text.length / 400));
  return `auto ${lines * 1.5}rem`;
}

/**
 * The conversation (UI design 2.4): prompts, Claude's turns, one trace line
 * per tool call with its output folded, and a day line where the date
 * changes. It sticks to the bottom unless you have scrolled up to read; a
 * button jumps back to the latest entry. Each agent keeps its place.
 */
export function ChatLog({ entries, tools, activity, idleSince, children }: ChatProps) {
  const now = useNow(30_000);
  const viewport = useRef<HTMLDivElement>(null);
  const agent = tools?.agent ?? "";
  const { turns, todo } = useMemo(() => {
    if (!entries) return { turns: [] as Turn[], todo: undefined };
    const rows = chatRows(entries, { thinking: true });
    // Only the latest todo list opens by itself.
    return { turns: turnsOf(groupRows(rows)), todo: lastToolKey(rows, "TodoWrite") };
  }, [entries]);

  // The cards read `tools` as they render; keep one object while its fields stay, so turns stay memoized.
  const latest = useRef(tools);
  latest.current = tools;
  const stable = useMemo<ToolContext | undefined>(
    () =>
      tools && {
        ...tools,
        onOpenAgent: tools.onOpenAgent && ((id: string) => latest.current?.onOpenAgent?.(id)),
      },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [tools?.agent, tools?.api, tools?.cwd, tools?.agents, !!tools?.onOpenAgent],
  );
  const watch = useMemo(() => makeWatch(() => viewport.current), [agent]); // eslint-disable-line react-hooks/exhaustive-deps

  // The long-press (or right-click on touch) menu acts on the turn under the finger.
  const [target, setTarget] = useState<Turn | null>(null);
  const touch = useRef(false);
  const pickTurn = (el: EventTarget) => {
    const key = (el as Element).closest?.("[data-turn]")?.getAttribute("data-turn");
    setTarget(turns.find((t) => t.key === key) ?? null);
  };

  const shown = entries !== undefined && (turns.length > 0 || !!children);
  useScrollMemory(agent, viewport, shown);

  if (entries === undefined) return <ChatSkeleton />;
  const live = activity === "working";
  const working = activity !== undefined && <WorkingRow activity={activity} idleSince={idleSince ?? null} />;
  if (!shown) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <Empty>
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <MessageSquareIcon />
            </EmptyMedia>
            <EmptyTitle>No messages yet</EmptyTitle>
            <EmptyDescription>The conversation shows up here as the agent works.</EmptyDescription>
          </EmptyHeader>
        </Empty>
        {working}
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <NowContext value={now}>
        <WatchContext value={watch}>
          <MessageScrollerProvider key={agent} defaultScrollPosition="end" autoScroll>
            <MessageScroller className="min-h-0 flex-1">
              <ContextMenu>
                <ContextMenuTrigger
                  className="contents"
                  onPointerDown={(e) => {
                    touch.current = e.pointerType === "touch";
                    pickTurn(e.target);
                  }}
                  onContextMenuCapture={(e) => {
                    // A mouse keeps the browser's own menu, for copying a selection.
                    if (!touch.current) e.stopPropagation();
                    else pickTurn(e.target);
                  }}
                >
                  <MessageScrollerViewport ref={viewport} role="log" aria-label="Conversation">
                    <MessageScrollerContent className="gap-4 px-3 py-3">
                      {turns.map((t, i) => (
                        <Fragment key={t.key}>
                          {(i === 0 || dayKey(turns[i - 1].ts) !== dayKey(t.ts)) && (
                            <MessageScrollerItem messageId={`day-${t.key}`}>
                              <DaySeparator ts={t.ts} />
                            </MessageScrollerItem>
                          )}
                          <MessageScrollerItem messageId={t.key} style={{ containIntrinsicSize: intrinsic(t) }}>
                            <TurnView
                              turn={t}
                              tools={stable}
                              open={todo !== undefined && t.items.some((x) => x.key === todo || (x.kind === "group" && x.rows.some((r) => r.key === todo))) ? todo : undefined}
                              live={live && i === turns.length - 1}
                              near={i >= turns.length - NEAR_END}
                            />
                          </MessageScrollerItem>
                        </Fragment>
                      ))}
                      {children && <MessageScrollerItem messageId="pending">{children}</MessageScrollerItem>}
                    </MessageScrollerContent>
                  </MessageScrollerViewport>
                </ContextMenuTrigger>
                <ContextMenuContent>
                  {MESSAGE_ACTIONS.map((a) => (
                    <ContextMenuItem key={a.id} disabled={!target?.text} onClick={() => target && a.run(target.text, agent)}>
                      <a.icon />
                      {a.label}
                    </ContextMenuItem>
                  ))}
                </ContextMenuContent>
              </ContextMenu>
              <MessageScrollerButton aria-label="Jump to latest" />
            </MessageScroller>
          </MessageScrollerProvider>
        </WatchContext>
      </NowContext>
      {working}
    </div>
  );
}
