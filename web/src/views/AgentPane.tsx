import {
  ChevronDownIcon,
  CopyIcon,
  EllipsisVerticalIcon,
  ExternalLinkIcon,
  GitBranchIcon,
  GitPullRequestIcon,
  PlayIcon,
  SquareIcon,
  SquareTerminalIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import type { Api } from "@/api/http";
import type { Activity, AgentView, ChatEntry, StateView, TaskView } from "@/api/types";
import type { Conn } from "@/conn";
import { useMedia, useModel } from "@/hooks";
import { KEYS, withCtrl } from "@/keys";
import type { AgentMode } from "@/route";
import type { Store } from "@/store";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { agentStatus, ciStatus, type Status } from "@/lib/status";
import { safeStorage } from "@/lib/storage";
import { cn } from "@/lib/utils";
import { ChatLog, Composer, queuedMessages } from "./Chat";
import { PendingRequest, pendingFor } from "./chat/Pending";
import { copyText } from "./chat/Turn";
import type { ToolContext } from "./chat/tools/parts";
import { SplitView, TermView } from "./lazy";
import { TokenUsageSummary } from "./shell/token-usage";
import { STATUS_TEXT, StatusIcon } from "./shell/status";
import { MiddlePath } from "./shell/path";

interface Props {
  view: StateView;
  agent: AgentView;
  /** From the URL; `null` picks the agent's last choice, then the device's default. */
  mode: AgentMode | null;
  phone: boolean;
  conn: Conn;
  store: Store;
  api: Api;
  readOnly?: boolean;
  onMode: (mode: AgentMode) => void;
  onOpenAgent: (agent: string) => void;
}

/** Split needs room for both: chat and terminal side by side. */
const SPLIT = "(min-width: 1280px)";
/** Sub-agent tabs beyond this go into a dropdown. */
const MAX_TABS = 4;

const modeKey = (agent: string) => `cmux.view.${agent}`;

function storedMode(agent: string): AgentMode | null {
  const m = safeStorage.getItem(modeKey(agent));
  return m === "chat" || m === "terminal" || m === "split" ? m : null;
}

/** The task agent and its sub-agents, as tabs; the executor alone. */
function siblings(v: StateView, a: AgentView): AgentView[] {
  const root = a.level === "sub" ? v.agents.find((x) => x.id === a.parent) : a;
  if (!root || root.level === "executor") return [a];
  return [root, ...v.agents.filter((x) => x.parent === root.id)];
}

function taskOf(v: StateView, a: AgentView): TaskView | undefined {
  if (!a.task) return undefined;
  return v.tasks.find((t) => t.id === a.task && a.id.startsWith(`${t.plan}/`)) ?? v.tasks.find((t) => t.id === a.task);
}

const tabName = (a: AgentView) => (a.level === "sub" ? a.path.slice(a.path.indexOf("/") + 1) : a.path);

function Dot({ status }: { status: Status }) {
  return (
    <span
      aria-hidden
      className={cn(
        "size-2 shrink-0 rounded-full bg-current",
        STATUS_TEXT[status.token],
        status.token === "working" && "animate-pulse",
      )}
    />
  );
}

/**
 * One agent (UI design 4.1 centre): its terminal, its chat, or both side by
 * side, with the message bar under them. Chat is the default on a phone.
 */
export function AgentPane({ view, agent, mode, phone, conn, store, api, readOnly, onMode, onOpenAgent }: Props) {
  const m = useModel(store);
  const splitOk = useMedia(SPLIT) && !phone;
  const [remembered, setRemembered] = useState(() => storedMode(agent.id));
  useEffect(() => {
    if (mode) {
      safeStorage.setItem(modeKey(agent.id), mode);
      setRemembered(mode);
    }
  }, [agent.id, mode]);
  let current: AgentMode = mode ?? remembered ?? (phone ? "chat" : "terminal");
  if (current === "split" && !splitOk) current = phone ? "chat" : "terminal";
  if (agent.term === null) current = "chat";
  const tabs = siblings(view, agent);
  const task = taskOf(view, agent);
  const status = agentStatus(agent);
  const exited = typeof agent.life === "object" || agent.life === "stopped";

  const choose = (x: AgentMode) => {
    safeStorage.setItem(modeKey(agent.id), x);
    setRemembered(x);
    onMode(x);
  };

  const chat = (
    <ChatPane
      agent={agent.id}
      conn={conn}
      entries={m.chats[agent.id]}
      activity={agent.life === "running" ? agent.activity : undefined}
      idleSince={agent.idle_since}
      tools={{ agent: agent.id, api, agents: view.agents, cwd: agent.cwd, onOpenAgent }}
    >
      {pendingFor(view, agent.id).map((n) => (
        <PendingRequest key={n.item.id} need={n} api={api} readOnly={readOnly} />
      ))}
    </ChatPane>
  );

  return (
    <section className="flex h-full min-h-0 min-w-0 flex-1 flex-col bg-background text-foreground">
      <header className="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1 border-b px-3 py-2">
        <span className="flex shrink-0 items-center gap-1.5 text-xs" title={status.label}>
          <Dot status={status} />
          <span className={STATUS_TEXT[status.token]}>{status.label}</span>
        </span>
        <h2 className="flex min-w-0 flex-1 font-mono text-sm">
          <MiddlePath value={agent.path} />
        </h2>
        {task?.branch && !phone && (
          <Button
            variant="outline"
            size="xs"
            className="max-w-48 font-mono"
            title="Copy the branch"
            onClick={() => void copyText(task.branch!, "branch")}
          >
            <GitBranchIcon data-icon="inline-start" />
            <span className="truncate">{task.branch}</span>
          </Button>
        )}
        {task?.pr && (
          <Button
            variant="outline"
            size="xs"
            nativeButton={false}
            render={<a href={task.pr.url} target="_blank" rel="noopener" />}
            title={task.pr.ci ? ciStatus(task.pr.ci).label : "No CI checks"}
          >
            <GitPullRequestIcon data-icon="inline-start" />#{task.pr.number}
            {task.pr.ci && <StatusIcon status={ciStatus(task.pr.ci)} />}
          </Button>
        )}
        <TokenUsageSummary tokens={agent.tokens} />
        <ToggleGroup
          aria-label="View"
          variant="outline"
          size="sm"
          spacing={0}
          value={[current]}
          onValueChange={(v: unknown[]) => v[0] && choose(v[0] as AgentMode)}
        >
          <ToggleGroupItem value="chat">Chat</ToggleGroupItem>
          <ToggleGroupItem value="terminal" disabled={agent.term === null}>
            Terminal
          </ToggleGroupItem>
          {splitOk && (
            <ToggleGroupItem value="split" disabled={agent.term === null}>
              Split
            </ToggleGroupItem>
          )}
        </ToggleGroup>
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" aria-label="Agent actions" />}>
            <EllipsisVerticalIcon />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem
              variant="destructive"
              disabled={readOnly || exited}
              onClick={() => api.stopAgent(agent.id).catch((e: unknown) => toast.error(String(e)))}
            >
              <SquareIcon />
              Stop
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={readOnly || !exited}
              onClick={() => api.resumeAgent(agent.id).catch((e: unknown) => toast.error(String(e)))}
            >
              <PlayIcon />
              Resume
            </DropdownMenuItem>
            <DropdownMenuItem disabled={agent.term === null} onClick={() => choose("terminal")}>
              <SquareTerminalIcon />
              Open terminal
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem disabled={!task?.branch} onClick={() => task?.branch && void copyText(task.branch, "branch")}>
              <CopyIcon />
              Copy branch
            </DropdownMenuItem>
            <DropdownMenuItem
              disabled={!task?.pr}
              onClick={() => task?.pr && window.open(task.pr.url, "_blank", "noopener")}
            >
              <ExternalLinkIcon />
              Open PR
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </header>
      {tabs.length > 1 && <AgentTabs tabs={tabs} current={agent.id} onOpen={onOpenAgent} />}
      {current === "split" && agent.term !== null ? (
        <SplitView
          chat={chat}
          term={<PhoneTerm term={agent.term} title={agent.id} phone={false} conn={conn} store={store} readOnly={readOnly} />}
        />
      ) : current === "terminal" && agent.term !== null ? (
        <PhoneTerm term={agent.term} title={agent.id} phone={phone} conn={conn} store={store} readOnly={readOnly} />
      ) : (
        chat
      )}
      <Composer agent={agent.id} api={api} queued={queuedMessages(view, agent.id)} readOnly={readOnly} />
    </section>
  );
}

/** The task's agents as a segmented control; past four, the rest sit in a dropdown. */
function AgentTabs({ tabs, current, onOpen }: { tabs: AgentView[]; current: string; onOpen: (id: string) => void }) {
  let shown = tabs.slice(0, MAX_TABS);
  if (!shown.some((a) => a.id === current)) shown = [...shown.slice(0, MAX_TABS - 1), tabs.find((a) => a.id === current)!];
  const more = tabs.filter((a) => !shown.includes(a));
  return (
    <nav className="flex shrink-0 items-center gap-1 overflow-x-auto border-b px-2 py-1" aria-label="Agents of this task">
      <ToggleGroup size="sm" spacing={1} value={[current]} onValueChange={(v: unknown[]) => v[0] && onOpen(v[0] as string)}>
        {shown.map((a) => (
          <ToggleGroupItem key={a.id} value={a.id} aria-current={a.id === current ? "page" : undefined} className="h-7 gap-1.5 px-2 text-xs">
            <Dot status={agentStatus(a)} />
            {tabName(a)}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      {more.length > 0 && (
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="xs" />}>
            {more.length} more
            <ChevronDownIcon data-icon="inline-end" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            {more.map((a) => (
              <DropdownMenuItem key={a.id} onClick={() => onOpen(a.id)}>
                <Dot status={agentStatus(a)} />
                {tabName(a)}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </nav>
  );
}

function ChatPane({
  agent,
  conn,
  entries,
  activity,
  idleSince,
  tools,
  children,
}: {
  agent: string;
  conn: Conn;
  entries: ChatEntry[] | undefined;
  activity: Activity | undefined;
  idleSince: number | null;
  tools: ToolContext;
  children: React.ReactNode[];
}) {
  useEffect(() => {
    conn.subChat(agent);
    return () => conn.unsubChat(agent);
  }, [agent, conn]);
  return (
    <ChatLog entries={entries} tools={tools} activity={activity} idleSince={idleSince}>
      {children.length > 0 && <div className="flex flex-col gap-2">{children}</div>}
    </ChatLog>
  );
}

interface TermProps {
  term: number;
  title: string;
  phone: boolean;
  conn: Conn;
  store: Store;
  readOnly?: boolean;
}

/** A key bar button: a 44 px touch target that does not shrink in the scrolling row. */
const KEY_CLASS = "h-11 min-w-11 shrink-0 snap-start bg-background px-2.5 text-sm";

/** The terminal, plus on a phone the key bar and "Fit to phone". */
export function PhoneTerm({ term, title, phone, conn, store, readOnly }: TermProps) {
  const [fit, setFit] = useState(false);
  const [ctrl, setCtrl] = useState(false);
  const ctrlRef = useRef(false);
  ctrlRef.current = ctrl;
  const filter = (d: string) => {
    if (!ctrlRef.current) return d;
    setCtrl(false);
    return withCtrl(d);
  };
  const press = (data: string) => conn.input(term, ctrlRef.current && data.length === 1 ? filter(data) : data);
  return (
    <>
      <TermView term={term} title={title} conn={conn} store={store} fit={!phone || fit} filter={filter} />
      {phone && (
        <div
          className="flex shrink-0 snap-x gap-1.5 overflow-x-auto overscroll-x-contain border-t bg-muted/40 py-1.5 pr-[max(0.5rem,env(safe-area-inset-right))] pl-[max(0.5rem,env(safe-area-inset-left))] [scrollbar-width:none]"
          role="toolbar"
          aria-label="Terminal keys"
        >
          {KEYS.map((k) =>
            k.data === undefined ? (
              <Button
                key={k.label}
                variant={ctrl ? "default" : "outline"}
                className={KEY_CLASS}
                aria-pressed={ctrl}
                title={k.title}
                disabled={readOnly}
                onClick={() => setCtrl((c) => !c)}
              >
                {k.label}
              </Button>
            ) : (
              <Button
                key={k.label}
                variant="outline"
                className={cn(KEY_CLASS, "font-mono")}
                title={k.title}
                disabled={readOnly}
                // Keep the terminal's focus (and the phone keyboard) up.
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => press(k.data!)}
              >
                {k.label}
              </Button>
            ),
          )}
          <Button
            variant={fit ? "secondary" : "outline"}
            className={cn(KEY_CLASS, "px-3")}
            aria-pressed={fit}
            onClick={() => setFit((f) => !f)}
            title="Resize the PTY to this screen"
          >
            Fit to phone
          </Button>
        </div>
      )}
    </>
  );
}
