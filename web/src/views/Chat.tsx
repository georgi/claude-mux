import { useEffect, useRef, useState } from "react";
import {
  ArrowUpIcon,
  ChevronsUpIcon,
  FileTextIcon,
  GlobeIcon,
  LoaderCircleIcon,
  MailIcon,
  MessageSquareIcon,
  SearchIcon,
  TerminalIcon,
  UsersIcon,
  WrenchIcon,
  CircleCheckIcon,
  FileSearchIcon,
} from "lucide-react";
import type { Api } from "@/api/http";
import type { ChatEntry } from "@/api/types";
import { chatRows, toolKind, type ChatRow, type ToolKind } from "@/chat";
import { Markdown } from "./lazy";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { CodeBlock, CodeBlockContent, CodeBlockCopyButton } from "@/components/ui/code-block";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupText, InputGroupTextarea } from "@/components/ui/input-group";
import { Message, MessageContent } from "@/components/ui/message";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/components/ui/message-scroller";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
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

interface ChatProps {
  /** `undefined` while the first batch is on its way. */
  entries: ChatEntry[] | undefined;
  /** Rendered as the last item, for example a pending permission request. */
  children?: React.ReactNode;
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

/** Long output clamps to the last lines; the block scrolls inside itself. */
const CLAMP = "max-h-64";

function ToolRow({ row }: { row: Extract<ChatRow, { kind: "tool" }> }) {
  const Icon = ICONS[toolKind(row.name)];
  const running = row.result === null;
  const line = (
    <TraceLine status={running ? "running" : undefined}>
      <TraceIcon>{running ? <LoaderCircleIcon className="animate-spin" /> : <Icon />}</TraceIcon>
      <TraceTitle>
        <span className="font-medium">{row.name}</span> <span className="font-mono text-xs">{row.summary}</span>
      </TraceTitle>
      {!running && (
        <TraceDescription>
          <CircleCheckIcon className="size-3.5" aria-label="Done" />
        </TraceDescription>
      )}
    </TraceLine>
  );
  if (running || !row.result) return <Trace>{line}</Trace>;
  return (
    <Trace>
      <TraceCollapsible>
        <TraceCollapsibleTrigger aria-label={`${row.name} output`}>{line}</TraceCollapsibleTrigger>
        <TraceCollapsibleContent>
          <CodeBlock size="sm">
            <CodeBlockCopyButton value={row.result} className="absolute end-1 top-1 z-10 bg-background" />
            <CodeBlockContent wrap={toolKind(row.name) !== "shell"} className={CLAMP}>
              {row.result}
            </CodeBlockContent>
          </CodeBlock>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}

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

/**
 * The conversation (UI design 2.4): prompts, Claude's text, one trace line per
 * tool call with its output folded. It sticks to the bottom unless you have
 * scrolled up to read; a button jumps back to the latest entry.
 */
export function ChatLog({ entries, children }: ChatProps) {
  if (entries === undefined) return <ChatSkeleton />;
  const rows = chatRows(entries);
  if (rows.length === 0 && !children) {
    return (
      <Empty>
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <MessageSquareIcon />
          </EmptyMedia>
          <EmptyTitle>No messages yet</EmptyTitle>
          <EmptyDescription>The conversation shows up here as the agent works.</EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }
  return (
    <MessageScrollerProvider defaultScrollPosition="end">
      <MessageScroller className="min-h-0 flex-1">
        <MessageScrollerViewport role="log" aria-label="Conversation">
          <MessageScrollerContent className="gap-4 px-3 py-3">
            {rows.map((r) => (
              <MessageScrollerItem key={r.key} messageId={r.key}>
                {r.kind === "tool" ? (
                  <ToolRow row={r} />
                ) : r.kind === "user" ? (
                  <Message align="end">
                    <MessageContent>
                      <Bubble align="end" variant="muted">
                        <BubbleContent className="whitespace-pre-wrap wrap-break-word">{r.text}</BubbleContent>
                      </Bubble>
                    </MessageContent>
                  </Message>
                ) : (
                  <Message>
                    <MessageContent>
                      <Markdown>{r.text}</Markdown>
                    </MessageContent>
                  </Message>
                )}
              </MessageScrollerItem>
            ))}
            {children && <MessageScrollerItem messageId="pending">{children}</MessageScrollerItem>}
          </MessageScrollerContent>
        </MessageScrollerViewport>
        <MessageScrollerButton aria-label="Jump to latest" />
      </MessageScroller>
    </MessageScrollerProvider>
  );
}

interface ComposerProps {
  agent: string;
  api: Api;
  /** Messages queued for this agent and not delivered yet. */
  queued: number;
  readOnly?: boolean;
}

/**
 * The message bar: **Enter** queues the message for the agent's next turn
 * boundary, **Steer** (or Ctrl/Cmd+Enter) interrupts it. Neither types into
 * the terminal.
 */
export function Composer({ agent, api, queued, readOnly }: ComposerProps) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);
  useEffect(() => setError(null), [agent]);

  const send = async (steer: boolean) => {
    const t = text.trim();
    if (!t || busy || readOnly) return;
    setBusy(true);
    setError(null);
    try {
      await api.message(agent, t, steer);
      setText("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
      area.current?.focus();
    }
  };

  const name = agent.slice(agent.indexOf("/") + 1);
  const empty = !text.trim();
  return (
    <form
      className="shrink-0 border-t bg-background p-2"
      onSubmit={(e) => {
        e.preventDefault();
        void send(false);
      }}
    >
      <InputGroup>
        <InputGroupTextarea
          ref={area}
          aria-label="Message"
          rows={1}
          className="field-sizing-content max-h-40 min-h-10"
          placeholder={readOnly ? "Read-only device" : `Message ${name}…`}
          disabled={readOnly}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void send(e.metaKey || e.ctrlKey);
            }
          }}
        />
        <InputGroupAddon align="block-end">
          {queued > 0 && (
            <InputGroupText>
              <MailIcon /> {queued} queued
            </InputGroupText>
          )}
          <Tooltip>
            <TooltipTrigger
              render={
                <InputGroupButton
                  type="button"
                  variant="outline"
                  size="sm"
                  className="ml-auto"
                  aria-label="Steer: interrupt the agent"
                  disabled={busy || readOnly || empty}
                  onClick={() => void send(true)}
                />
              }
            >
              <ChevronsUpIcon /> Steer
            </TooltipTrigger>
            <TooltipContent>Interrupt the agent (Ctrl+Enter)</TooltipContent>
          </Tooltip>
          <InputGroupButton type="submit" variant="default" size="icon-sm" aria-label="Send at the next turn" disabled={busy || readOnly || empty}>
            <ArrowUpIcon />
          </InputGroupButton>
        </InputGroupAddon>
      </InputGroup>
      {error && (
        <p role="alert" className="mt-1 text-xs text-destructive">
          {error}
        </p>
      )}
    </form>
  );
}
