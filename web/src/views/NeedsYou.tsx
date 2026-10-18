import { useState } from "preact/hooks";
import type { Api } from "../api/http";
import type { StateView } from "../api/types";
import { type Action, type Nav, type Need, needs, perform } from "../needs";

interface Props {
  view: StateView;
  api: Api;
  nav: Nav;
  readOnly?: boolean;
  /** Milliseconds since the epoch, for the age labels (tests pin it). */
  now?: number;
}

function age(created: number, now: number): string {
  const s = Math.max(0, Math.floor((now - created) / 1000));
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

/** The needs-you list (UI design 2.3), each item with its one-tap actions. */
export function NeedsYou({ view, api, nav, readOnly, now }: Props) {
  const list = needs(view);
  const t = now ?? Date.now();
  if (list.length === 0) return <p class="muted pad">Nothing needs you.</p>;
  return (
    <ul class="needs" aria-label="Needs you">
      {list.map((n) => (
        <NeedCard key={n.item.id} need={n} api={api} nav={nav} readOnly={readOnly} age={age(n.item.created, t)} />
      ))}
    </ul>
  );
}

function NeedCard({ need, api, nav, readOnly, age }: { need: Need; api: Api; nav: Nav; readOnly?: boolean; age: string }) {
  const [asking, setAsking] = useState<Action | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (a: Action, t = "") => {
    setBusy(true);
    setError(null);
    try {
      await perform(api, nav, need, a.id, t);
      setAsking(null);
      setText("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const tap = (a: Action) => {
    if (a.text) {
      setAsking(a);
      setError(null);
      return;
    }
    void run(a);
  };

  const submit = (ev: Event) => {
    ev.preventDefault();
    if (asking) void run(asking, text);
  };

  const local = (a: Action) => a.id === "open" || a.id === "open_pr";
  const glyph = need.item.kind === "stalled" ? "⚠" : "⚑";
  return (
    <li class="need" data-item={need.item.id}>
      <div class="need-head">
        <span class="glyph ask">{glyph}</span>
        <strong>{need.who}</strong>
        <span class="muted small">· {age}</span>
      </div>
      <div class="need-title">{need.title}</div>
      {need.detail && <code class="need-detail">{need.detail}</code>}
      {asking ? (
        <form class="need-form" onSubmit={submit}>
          <textarea
            aria-label={asking.text === "note" ? "Note" : asking.label.replace("…", "")}
            placeholder={asking.text === "note" ? "Optional note" : "Text"}
            value={text}
            rows={2}
            onInput={(e) => setText((e.target as HTMLTextAreaElement).value)}
            ref={(el) => el?.focus()}
          />
          <div class="actions">
            <button type="submit" disabled={busy || (asking.text === "required" && !text.trim())}>
              {asking.label.replace("…", "")}
            </button>
            <button type="button" class="ghost" onClick={() => setAsking(null)}>
              Cancel
            </button>
          </div>
        </form>
      ) : (
        <div class="actions">
          {need.actions.map((a) => (
            <button
              key={a.id}
              type="button"
              class={a.primary ? "" : "ghost"}
              disabled={busy || (readOnly && !local(a))}
              onClick={() => tap(a)}
            >
              {a.label}
            </button>
          ))}
        </div>
      )}
      {error && <p class="error small">{error}</p>}
    </li>
  );
}
