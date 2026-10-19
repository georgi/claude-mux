import { PlusIcon, SquareMousePointerIcon } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarInset,
  SidebarProvider,
} from "@/components/ui/sidebar";
import { Skeleton } from "@/components/ui/skeleton";
import { Toaster } from "@/components/ui/sonner";
import { api, endSession, hasSession } from "../api/http";
import { Conn, streamUrl } from "../conn";
import { useMedia, useModel } from "../hooks";
import { type Nav, pending } from "../needs";
import { formatRoute, parseRoute, type Route } from "../route";
import { Store } from "../store";
import { AgentPane } from "./AgentPane";
import { NeedsYou } from "./NeedsYou";
import { Pairing } from "./Pairing";
import { Keyboard } from "./shell/command";
import { crumbsFor, Header } from "./shell/header";
import { Live } from "./shell/live";
import { NeedsLayout } from "./shell/needs-aside";
import { PhoneTabs, type Tab } from "./shell/phone-tabs";
import { Board, NewPlan, PlanList, Settings, TermView } from "./lazy";
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

  let body: React.ReactNode;
  if (error)
    body = (
      <Empty className="min-h-svh">
        <EmptyHeader>
          <EmptyTitle>Cannot reach the server</EmptyTitle>
          <EmptyDescription className="break-all">{error}</EmptyDescription>
        </EmptyHeader>
        <EmptyContent>
          <Button
            onClick={() => {
              setError(null);
              setPhase("checking");
            }}
          >
            Retry
          </Button>
        </EmptyContent>
      </Empty>
    );
  else if (phase === "checking")
    body = (
      <main className="flex min-h-svh flex-col items-center justify-center gap-3" aria-busy="true">
        <Skeleton className="h-6 w-40" />
        <p className="text-sm text-muted-foreground">Connecting…</p>
      </main>
    );
  else if (phase === "pair") body = <Pairing onPaired={() => setPhase("app")} />;
  else body = <Main onSignedOut={() => setPhase("pair")} />;

  return (
    <>
      {body}
      <Toaster position="top-center" />
    </>
  );
}

/** Phones get bottom tabs and chat first (UI design 4.2). */
const PHONE = "(max-width: 639px)";
/** Wide screens show the needs-you column (UI design 4.1). */
const WIDE = "(min-width: 1024px)";

function Loading() {
  return (
    <div className="flex flex-col gap-2 p-4" aria-busy="true">
      <Skeleton className="h-5 w-1/2" />
      <Skeleton className="h-5 w-2/3" />
      <Skeleton className="h-5 w-1/3" />
    </div>
  );
}

function Missing({ text }: { text: string }) {
  return (
    <Empty>
      <EmptyHeader>
        <EmptyTitle>Not found</EmptyTitle>
        <EmptyDescription>{text}</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}

function Main({ onSignedOut }: { onSignedOut: () => void }) {
  const store = useMemo(() => new Store(), []);
  const conn = useMemo(() => new Conn(store, { url: streamUrl(location), info: navigator.userAgent }), [store]);
  const m = useModel(store);
  const [route, setRoute] = useState<Route>(parseRoute(location.hash));
  const phone = useMedia(PHONE);
  const wide = useMedia(WIDE);
  const seen = useRef(false);
  if (m.conn === "open") seen.current = true;

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

  useEffect(() => {
    if (m.error && m.error.code !== "unauthorized") toast.error(m.error.message, { id: "server-error" });
  }, [m.error]);

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
    if (!v) return <Loading />;
    switch (route.view) {
      case "agent": {
        const a = v.agents.find((x) => x.id === route.agent);
        if (!a) return <Missing text={`No agent ${route.agent}.`} />;
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
        if (!t) return <Missing text={`No terminal ${route.term}.`} />;
        return (
          <div className="min-h-0 flex-1">
            <TermView key={t.id} term={t.id} title={t.title ?? `terminal ${t.id}`} conn={conn} store={store} />
          </div>
        );
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
        return (
          <Empty>
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <SquareMousePointerIcon />
              </EmptyMedia>
              <EmptyTitle>No agent open</EmptyTitle>
              <EmptyDescription>Open an agent from the tree.</EmptyDescription>
            </EmptyHeader>
          </Empty>
        );
    }
  };

  const tab: Tab =
    route.view === "home" || route.view === "needs"
      ? "needs"
      : route.view === "plans" || route.view === "board" || route.view === "new"
        ? "plans"
        : route.view === "settings"
          ? "settings"
          : "live";

  return (
    <SidebarProvider defaultOpen={matchMedia("(min-width: 768px)").matches} className="h-svh overflow-hidden">
      <Sidebar aria-label="Agents and plans">
        <SidebarHeader className="flex-row items-center justify-between">
          <span className="px-2 text-sm font-semibold">{m.server?.name ?? "cmux"}</span>
        </SidebarHeader>
        <SidebarContent>
          <nav aria-label="Agents and plans">
            {v ? (
              <Tree
                view={v}
                selected={selected}
                onOpen={(r) => (r.agent ? nav.openAgent(r.agent) : r.term !== undefined && go({ view: "term", term: r.term }))}
                onPlan={nav.openPlan}
              />
            ) : (
              <div className="p-3">
                <Loading />
              </div>
            )}
          </nav>
        </SidebarContent>
        <SidebarFooter>
          <Button variant="outline" size="sm" onClick={() => go({ view: "new" })}>
            <PlusIcon data-icon="inline-start" />
            New plan
          </Button>
        </SidebarFooter>
      </Sidebar>
      <SidebarInset className="min-w-0 overflow-hidden">
        <Header
          m={m}
          crumbs={crumbsFor(v, route)}
          seen={seen.current}
          phone={phone}
          wide={wide}
          readOnly={readOnly}
          needsCount={count}
          onNeeds={() => go({ view: "needs" })}
          onNew={() => go({ view: "new" })}
          onSettings={() => go({ view: "settings" })}
          onSignOut={signOut}
        />
        {wide && !phone ? (
          <NeedsLayout count={count} main={body()}>
            {v ? <NeedsYou view={v} api={api} nav={nav} readOnly={readOnly} /> : <Loading />}
          </NeedsLayout>
        ) : (
          <div className="flex min-h-0 flex-1">
            <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">{body()}</main>
          </div>
        )}
        {phone && <PhoneTabs tab={tab} count={count} go={(t) => go({ view: t })} />}
      </SidebarInset>
      <Keyboard view={v} route={route} readOnly={readOnly} api={api} nav={nav} go={go} onSignOut={signOut} />
    </SidebarProvider>
  );
}
