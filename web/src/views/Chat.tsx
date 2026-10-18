import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import type { Api } from "../api/http";
import type { ChatEntry } from "../api/types";
import { chatRows } from "../chat";

interface ChatProps {
  /** `undefined` while the first batch is on its way. */
  entries: ChatEntry[] | undefined;
}

/**
 * The conversation (UI design 2.4): prompts, Claude's text, one line per
 * tool call with its output folded. It sticks to the bottom unless you have
 * scrolled up to read.
 */
export function ChatLog({ entries }: ChatProps) {
  const box = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  useLayoutEffect(() => {
    const el = box.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [entries]);
  const onScroll = () => {
    const el = box.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  };
  if (entries === undefined) return <p class="muted pad">Loading the conversation…</p>;
  const rows = chatRows(entries);
  return (
    <div class="chat" ref={box} onScroll={onScroll} role="log" aria-label="Conversation">
      {rows.length === 0 && <p class="muted">No messages yet.</p>}
      {rows.map((r) =>
        r.kind === "tool" ? (
          <details key={r.key} class="chat-tool">
            <summary>
              <span class="muted">▸</span> <strong>{r.name}</strong> <code>{r.summary}</code>{" "}
              {r.result === null ? <span class="muted">…</span> : <span class="ok">✓</span>}
            </summary>
            {r.result !== null && <pre>{r.result}</pre>}
          </details>
        ) : (
          <div key={r.key} class={`chat-msg ${r.kind}`}>
            {r.kind === "user" && <span class="muted">› </span>}
            {r.text}
          </div>
        ),
      )}
    </div>
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
 * The message bar: **Queue** delivers at the agent's next turn boundary,
 * **Steer** interrupts it. Neither types into the terminal.
 */
export function Composer({ agent, api, queued, readOnly }: ComposerProps) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setError(null), [agent]);

  const send = async (steer: boolean) => {
    const t = text.trim();
    if (!t) return;
    setBusy(true);
    setError(null);
    try {
      await api.message(agent, t, steer);
      setText("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const name = agent.slice(agent.indexOf("/") + 1);
  return (
    <form
      class="composer"
      onSubmit={(e) => {
        e.preventDefault();
        void send(false);
      }}
    >
      {queued > 0 && <span class="queued small">✉ {queued} queued</span>}
      <textarea
        aria-label="Message"
        rows={1}
        placeholder={readOnly ? "Read-only device" : `Message ${name}…`}
        disabled={readOnly}
        value={text}
        onInput={(e) => setText((e.target as HTMLTextAreaElement).value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            void send(e.metaKey || e.ctrlKey);
          }
        }}
      />
      <button type="submit" disabled={busy || readOnly || !text.trim()} title="Deliver at the next turn (Enter)">
        Queue
      </button>
      <button
        type="button"
        class="ghost"
        disabled={busy || readOnly || !text.trim()}
        title="Interrupt the agent (Ctrl+Enter)"
        onClick={() => void send(true)}
      >
        Steer
      </button>
      {error && <p class="error small">{error}</p>}
    </form>
  );
}
