import { useEffect, useMemo, useState } from "preact/hooks";
import { api, endSession, hasSession } from "../api/http";
import type { StateView } from "../api/types";
import { Conn, streamUrl } from "../conn";
import { useMedia, useModel, useSpinner } from "../hooks";
import { type Nav, pending } from "../needs";
import { formatRoute, parseRoute, type Route } from "../route";
import { Store } from "../store";
import { agentGlyph } from "../tree";
import { AgentPane } from "./AgentPane";
import { Board, NewPlan, PlanList } from "./Board";
import { NeedsYou } from "./NeedsYou";
import { Pairing } from "./Pairing";
import { Settings } from "./Settings";
import { TermView } from "./TermView";
import { Tree } from "./Tree";

type Phase = "checking" | "pair" | "app";

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

/** Phones get bottom tabs and chat first (UI design 4.2). */
const PHONE = "(max-width: 639px)";
/** Wide screens show the needs-you column (UI design 4.1). */
const WIDE = "(min-width: 1024px)";

function Main({ onSignedOut }: { onSignedOut: () => void }) {
  const store = useMemo(() => new Store(), []);
  const conn = useMemo(() => new Conn(store, { url: streamUrl(location), info: navigator.userAgent }), [store]);
  const m = useModel(store);
  const [route, setRoute] = useState<Route>(parseRoute(location.hash));
  const phone = useMedia(PHONE);
  const wide = useMedia(WIDE);

  useEffect(() => {
    conn.start();
    const kick = () => conn.kick();
    const visible = () => document.visibilityState === "visible" && conn.kick();
    const hash = () => setRoute(parseRoute(location.hash));
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

  const go = (r: Route) => {
    location.hash = formatRoute(r);
    setRoute(r);
  };
  const nav: Nav = {
    openAgent: (agent) => go({ view: "agent", agent, mode: null }),
    openPlan: (plan) => go({ view: "board", plan }),
    openUrl: (url) => window.open(url, "_blank", "noopener"),
  };

  const v = m.view;
  const readOnly = m.scope === "read";
  const count = v ? pending(v).length : 0;
  const selected = route.view === "agent" ? route.agent : route.view === "term" ? `term-${route.term}` : null;

  const body = () => {
    if (!v) return <p class="muted pad">Loading…</p>;
    switch (route.view) {
      case "agent": {
        const a = v.agents.find((x) => x.id === route.agent);
        if (!a) return <p class="muted pad">No agent {route.agent}.</p>;
        return (
          <AgentPane
            key={a.id}
            view={v}
            agent={a}
            mode={route.mode}
            phone={phone}
            conn={conn}
            store={store}
            api={api}
            readOnly={readOnly}
            onMode={(mode) => go({ view: "agent", agent: a.id, mode })}
            onOpenAgent={nav.openAgent}
          />
        );
      }
      case "term": {
        const t = v.terminals.find((x) => x.id === route.term);
        if (!t) return <p class="muted pad">No terminal {route.term}.</p>;
        return <TermView key={t.id} term={t.id} title={t.title ?? `terminal ${t.id}`} conn={conn} store={store} />;
      }
      case "board":
        return <Board view={v} plan={route.plan} api={api} readOnly={readOnly} onOpenAgent={nav.openAgent} />;
      case "new":
        return <NewPlan api={api} readOnly={readOnly} onCreated={(plan) => go({ view: "board", plan })} />;
      case "settings":
        return <Settings api={api} readOnly={readOnly} />;
      case "plans":
        return <PlanList view={v} onOpen={nav.openPlan} onNew={() => go({ view: "new" })} />;
      case "live":
        return <Live view={v} onOpen={nav.openAgent} />;
      case "needs":
        return <NeedsYou view={v} api={api} nav={nav} readOnly={readOnly} />;
      case "home":
        if (phone) return <NeedsYou view={v} api={api} nav={nav} readOnly={readOnly} />;
        return <p class="muted pad">Open an agent from the tree.</p>;
    }
  };

  const tab = route.view === "home" || route.view === "needs" ? "needs" : route.view === "plans" || route.view === "board" || route.view === "new" ? "plans" : route.view === "settings" ? "settings" : "live";

  return (
    <div class={`app${phone ? " phone" : ""}${wide ? " wide" : ""}`}>
      <header class="top">
        <span class={`dot ${m.conn}`} title={m.conn} />
        <strong>{m.server?.name ?? "cmux"}</strong>
        <span class="muted small">
          {m.conn === "open" ? `${m.presence.length} client${m.presence.length === 1 ? "" : "s"}` : m.conn}
          {readOnly ? " · read-only" : ""}
        </span>
        <span class="grow" />
        {m.error && m.error.code !== "unauthorized" && <span class="error small">{m.error.message}</span>}
        {!phone && !wide && (
          <button class="link" onClick={() => go({ view: "needs" })}>
            Needs you ({count})
          </button>
        )}
        {!phone && (
          <button class="link" onClick={() => go({ view: "new" })}>
            New plan
          </button>
        )}
        <button class="link" onClick={() => go({ view: "settings" })} aria-label="Settings">
          Settings
        </button>
        <button class="link" onClick={signOut}>
          Sign out
        </button>
      </header>
      {!phone && (
        <aside class="side">
          {v ? (
            <Tree
              view={v}
              selected={selected}
              onOpen={(r) => (r.agent ? nav.openAgent(r.agent) : r.term !== undefined && go({ view: "term", term: r.term }))}
              onPlan={nav.openPlan}
            />
          ) : (
            <p class="muted pad">Loading…</p>
          )}
        </aside>
      )}
      <main class="center">{body()}</main>
      {wide && !phone && (
        <aside class="needs-col">
          <h3 class="pad-x">Needs you ({count})</h3>
          {v && <NeedsYou view={v} api={api} nav={nav} readOnly={readOnly} />}
        </aside>
      )}
      {phone && (
        <nav class="tabs" aria-label="Sections">
          <button class={tab === "needs" ? "on" : ""} onClick={() => go({ view: "needs" })}>
            Needs you{count ? <span class="count">{count}</span> : null}
          </button>
          <button class={tab === "plans" ? "on" : ""} onClick={() => go({ view: "plans" })}>
            Plans
          </button>
          <button class={tab === "live" ? "on" : ""} onClick={() => go({ view: "live" })}>
            Live
          </button>
        </nav>
      )}
    </div>
  );
}

/** The phone's Live tab: agents running now, working ones first. */
function Live({ view, onOpen }: { view: StateView; onOpen: (agent: string) => void }) {
  const live = view.agents.filter((a) => a.life === "running" || a.life === "starting");
  const spin = useSpinner(live.some((a) => a.activity === "working"));
  const rank = (a: (typeof live)[number]) => (a.activity === "needs_permission" ? 0 : a.activity === "working" ? 1 : 2);
  const plans = new Map(view.plans.map((p) => [p.id, p.title]));
  if (live.length === 0) return <p class="muted pad">No agent is running.</p>;
  return (
    <ul class="plain live">
      {[...live]
        .sort((a, b) => rank(a) - rank(b))
        .map((a) => (
          <li key={a.id}>
            <button class="row-button" onClick={() => onOpen(a.id)}>
              <span class="glyph">{agentGlyph(a, spin)}</span>
              <span class="label">{a.path}</span>
              <span class="muted small">{plans.get(a.id.slice(0, a.id.indexOf("/"))) ?? ""}</span>
            </button>
          </li>
        ))}
    </ul>
  );
}
