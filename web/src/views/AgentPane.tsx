import { useEffect, useRef, useState } from "react";
import type { Api } from "@/api/http";
import type { AgentView, ChatEntry, StateView } from "@/api/types";
import type { Conn } from "@/conn";
import { useModel, useSpinner } from "@/hooks";
import { KEYS, withCtrl } from "@/keys";
import type { AgentMode } from "@/route";
import type { Store } from "@/store";
import { agentGlyph } from "@/tree";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { ChatLog, Composer } from "./Chat";
import { PendingRequest, pendingFor } from "./chat/Pending";
import type { ToolContext } from "./chat/tools/parts";
import { TermView } from "./lazy";

interface Props {
  view: StateView;
  agent: AgentView;
  /** From the URL; `null` picks the device's default. */
  mode: AgentMode | null;
  phone: boolean;
  conn: Conn;
  store: Store;
  api: Api;
  readOnly?: boolean;
  onMode: (mode: AgentMode) => void;
  onOpenAgent: (agent: string) => void;
}

/** Messages queued for `agent` and not delivered yet (the `✉N` count). */
export function queuedFor(v: StateView, agent: string): number {
  return v.items.filter(
    (i) =>
      typeof i.to === "object" &&
      i.to.agent === agent &&
      i.status === "pending" &&
      (i.kind === "user_message" || i.kind === "steer"),
  ).length;
}

/** The task agent and its sub-agents, as tabs; the executor alone. */
function siblings(v: StateView, a: AgentView): AgentView[] {
  const root = a.level === "sub" ? v.agents.find((x) => x.id === a.parent) : a;
  if (!root || root.level === "executor") return [a];
  return [root, ...v.agents.filter((x) => x.parent === root.id)];
}

/**
 * One agent (UI design 4.1 centre): its terminal or its chat, with the
 * message bar under both. Chat is the default on a phone.
 */
export function AgentPane({ view, agent, mode, phone, conn, store, api, readOnly, onMode, onOpenAgent }: Props) {
  const m = useModel(store);
  const spin = useSpinner(agent.activity === "working");
  const current: AgentMode = mode ?? (phone ? "chat" : "terminal");
  const tabs = siblings(view, agent);
  const name = (a: AgentView) => (a.level === "sub" ? a.path.slice(a.path.indexOf("/") + 1) : a.path);

  return (
    <section className="flex h-full min-h-0 min-w-0 flex-1 flex-col bg-background text-foreground">
      <header className="flex shrink-0 items-center gap-2 border-b px-3 py-2">
        <span className="font-mono text-sm" aria-hidden="true">
          {agentGlyph(agent, spin)}
        </span>
        <h2 className="min-w-0 flex-1 truncate text-sm font-medium">{agent.path}</h2>
        <div className="flex gap-1" role="tablist" aria-label="View">
          {(["chat", "terminal"] as const).map((x) => (
            <Button
              key={x}
              role="tab"
              size="sm"
              variant={current === x ? "secondary" : "ghost"}
              aria-selected={current === x}
              onClick={() => onMode(x)}
              disabled={x === "terminal" && agent.term === null}
            >
              {x === "chat" ? "Chat" : "Terminal"}
            </Button>
          ))}
        </div>
      </header>
      {tabs.length > 1 && (
        <nav className="flex shrink-0 gap-1 overflow-x-auto border-b px-2 py-1" aria-label="Agents of this task">
          {tabs.map((a) => (
            <Button
              key={a.id}
              size="xs"
              variant={a.id === agent.id ? "secondary" : "ghost"}
              aria-current={a.id === agent.id ? "page" : undefined}
              onClick={() => onOpenAgent(a.id)}
            >
              {agentGlyph(a, spin)} {name(a)}
            </Button>
          ))}
        </nav>
      )}
      {current === "terminal" && agent.term !== null ? (
        <PhoneTerm term={agent.term} title={agent.id} phone={phone} conn={conn} store={store} readOnly={readOnly} />
      ) : (
        <ChatPane
          agent={agent.id}
          conn={conn}
          entries={m.chats[agent.id]}
          tools={{ agent: agent.id, api, agents: view.agents, cwd: agent.cwd, onOpenAgent }}
        >
          {pendingFor(view, agent.id).map((n) => (
            <PendingRequest key={n.item.id} need={n} api={api} />
          ))}
        </ChatPane>
      )}
      <Composer agent={agent.id} api={api} queued={queuedFor(view, agent.id)} readOnly={readOnly} />
    </section>
  );
}

function ChatPane({
  agent,
  conn,
  entries,
  tools,
  children,
}: {
  agent: string;
  conn: Conn;
  entries: ChatEntry[] | undefined;
  tools: ToolContext;
  children: React.ReactNode[];
}) {
  useEffect(() => {
    conn.subChat(agent);
    return () => conn.unsubChat(agent);
  }, [agent, conn]);
  return (
    <ChatLog entries={entries} tools={tools}>
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
        <div className="flex shrink-0 gap-1 overflow-x-auto border-t p-1" role="toolbar" aria-label="Terminal keys">
          {KEYS.map((k) =>
            k.data === undefined ? (
              <Button
                key={k.label}
                size="sm"
                variant={ctrl ? "secondary" : "outline"}
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
                size="sm"
                variant="outline"
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
            size="sm"
            variant={fit ? "secondary" : "outline"}
            className={cn("shrink-0")}
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
