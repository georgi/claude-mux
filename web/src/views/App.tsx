import { useEffect, useMemo, useState } from "preact/hooks";
import { endSession, hasSession } from "../api/http";
import { Conn, streamUrl } from "../conn";
import { useModel } from "../hooks";
import { Store } from "../store";
import { Pairing } from "./Pairing";
import { TermView } from "./TermView";
import { Tree } from "./Tree";

type Phase = "checking" | "pair" | "app";

/** The open terminal lives in the hash (`#term=3`), so reload keeps it. */
function termFromHash(): number | null {
  const m = /term=(\d+)/.exec(location.hash);
  return m ? Number(m[1]) : null;
}

export function App() {
  const [phase, setPhase] = useState<Phase>(location.pathname === "/pair" ? "pair" : "checking");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (phase !== "checking") return;
    hasSession()
      .then((ok) => setPhase(ok ? "app" : "pair"))
      .catch((e: unknown) => setError(String(e)));
  }, [phase]);

  if (error) return <p class="error pad">Cannot reach the server: {error}</p>;
  if (phase === "checking") return <p class="muted pad">Connecting…</p>;
  if (phase === "pair") return <Pairing onPaired={() => setPhase("app")} />;
  return <Main onSignedOut={() => setPhase("pair")} />;
}

function Main({ onSignedOut }: { onSignedOut: () => void }) {
  const store = useMemo(() => new Store(), []);
  const conn = useMemo(() => new Conn(store, { url: streamUrl(location), info: navigator.userAgent }), [store]);
  const m = useModel(store);
  const [open, setOpen] = useState<number | null>(termFromHash());

  useEffect(() => {
    conn.start();
    const kick = () => conn.kick();
    const visible = () => document.visibilityState === "visible" && conn.kick();
    const hash = () => setOpen(termFromHash());
    window.addEventListener("online", kick);
    document.addEventListener("visibilitychange", visible);
    window.addEventListener("hashchange", hash);
    return () => {
      window.removeEventListener("online", kick);
      document.removeEventListener("visibilitychange", visible);
      window.removeEventListener("hashchange", hash);
      conn.stop();
    };
  }, [conn]);

  useEffect(() => {
    if (m.conn === "unauthorized") {
      conn.stop();
      onSignedOut();
    }
  }, [m.conn]);

  const signOut = async () => {
    conn.stop();
    await endSession().catch(() => {});
    onSignedOut();
  };

  const openTerm = (t: number) => {
    location.hash = `term=${t}`;
    setOpen(t);
  };

  const v = m.view;
  const agent = v?.agents.find((a) => a.term === open);
  const terminal = v?.terminals.find((t) => t.id === open);
  const title = agent ? agent.id : terminal ? (terminal.title ?? `terminal ${terminal.id}`) : "";

  return (
    <div class="app">
      <header class="top">
        <span class={`dot ${m.conn}`} title={m.conn} />
        <strong>{m.server?.name ?? "cmux"}</strong>
        <span class="muted small">
          {m.conn === "open" ? `${m.presence.length} client${m.presence.length === 1 ? "" : "s"}` : m.conn}
          {m.scope === "read" ? " · read-only" : ""}
        </span>
        <span class="grow" />
        {m.error && m.error.code !== "unauthorized" && <span class="error small">{m.error.message}</span>}
        <button class="link" onClick={signOut}>
          Sign out
        </button>
      </header>
      <aside class="side">{v ? <Tree view={v} selected={open} onOpen={openTerm} /> : <p class="muted pad">Loading…</p>}</aside>
      <main class="center">
        {open !== null && v && (agent || terminal) ? (
          <TermView key={open} term={open} title={title} conn={conn} store={store} />
        ) : (
          <p class="muted pad">Open an agent from the tree.</p>
        )}
      </main>
    </div>
  );
}
