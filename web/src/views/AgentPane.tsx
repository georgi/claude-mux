import { useEffect, useRef, useState } from "preact/hooks";
import type { Api } from "../api/http";
import type { AgentView, StateView } from "../api/types";
import type { Conn } from "../conn";
import { useModel, useSpinner } from "../hooks";
import { KEYS, withCtrl } from "../keys";
import type { AgentMode } from "../route";
import type { Store } from "../store";
import { agentGlyph } from "../tree";
import { ChatLog, Composer } from "./Chat";
import { TermView } from "./TermView";

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
    <section class="agent">
      <header class="agent-head">
        <span class="glyph">{agentGlyph(agent, spin)}</span>
        <span class="title">{agent.path}</span>
        <span class="grow" />
        <div class="toggle" role="tablist" aria-label="View">
          {(["chat", "terminal"] as const).map((x) => (
            <button
              key={x}
              role="tab"
              aria-selected={current === x}
              class={current === x ? "on" : ""}
              onClick={() => onMode(x)}
              disabled={x === "terminal" && agent.term === null}
            >
              {x === "chat" ? "Chat" : "Terminal"}
            </button>
          ))}
        </div>
      </header>
      {tabs.length > 1 && (
        <nav class="subtabs">
          {tabs.map((a) => (
            <button key={a.id} class={a.id === agent.id ? "on" : ""} onClick={() => onOpenAgent(a.id)}>
              {agentGlyph(a, spin)} {name(a)}
            </button>
          ))}
        </nav>
      )}
      {current === "terminal" && agent.term !== null ? (
        <PhoneTerm term={agent.term} title={agent.id} phone={phone} conn={conn} store={store} readOnly={readOnly} />
      ) : (
        <ChatPane agent={agent.id} conn={conn} entries={m.chats[agent.id]} />
      )}
      <Composer agent={agent.id} api={api} queued={queuedFor(view, agent.id)} readOnly={readOnly} />
    </section>
  );
}

function ChatPane({ agent, conn, entries }: { agent: string; conn: Conn; entries: Parameters<typeof ChatLog>[0]["entries"] }) {
  useEffect(() => {
    conn.subChat(agent);
    return () => conn.unsubChat(agent);
  }, [agent, conn]);
  return <ChatLog entries={entries} />;
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
        <div class="keybar" role="toolbar" aria-label="Terminal keys">
          {KEYS.map((k) =>
            k.data === undefined ? (
              <button
                key={k.label}
                class={ctrl ? "on" : ""}
                aria-pressed={ctrl}
                title={k.title}
                disabled={readOnly}
                onClick={() => setCtrl((c) => !c)}
              >
                {k.label}
              </button>
            ) : (
              <button
                key={k.label}
                title={k.title}
                disabled={readOnly}
                // Keep the terminal's focus (and the phone keyboard) up.
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => press(k.data!)}
              >
                {k.label}
              </button>
            ),
          )}
          <button class={fit ? "on" : ""} aria-pressed={fit} onClick={() => setFit((f) => !f)} title="Resize the PTY to this screen">
            Fit to phone
          </button>
        </div>
      )}
    </>
  );
}
