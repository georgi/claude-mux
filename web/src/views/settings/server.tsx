import { RefreshCwIcon } from "lucide-react";
import { useEffect } from "react";
import type { Api } from "../../api/http";
import type { AgentStatus, SettingsResp, StatusResp } from "../../api/types";
import { Err, KV, Section, useLoad } from "./common";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from "@/components/ui/item";
import { Skeleton } from "@/components/ui/skeleton";

export function uptime(s: number): string {
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`;
}

function mb(kb: number): string {
  return kb >= 1024 * 1024 ? `${(kb / 1024 / 1024).toFixed(1)} GB` : `${Math.round(kb / 1024)} MB`;
}

/** Largest first; agents without a reading go last. */
export function byRss(agents: AgentStatus[]): AgentStatus[] {
  return agents.slice().sort((a, b) => (b.rss_kb ?? -1) - (a.rss_kb ?? -1) || a.agent.localeCompare(b.agent));
}

/** Per-agent memory as a list with inline bars, scaled to the largest. */
export function RssList({ agents }: { agents: AgentStatus[] }) {
  const sorted = byRss(agents);
  const max = Math.max(1, ...sorted.map((a) => a.rss_kb ?? 0));
  if (sorted.length === 0) return <p className="text-sm text-muted-foreground">No live agents.</p>;
  return (
    <ul aria-label="Memory by agent" className="flex flex-col gap-2">
      {sorted.map((a) => (
        <li key={a.agent} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 text-sm">
          <code className="truncate text-xs" title={a.agent}>
            {a.agent.slice(a.agent.indexOf("/") + 1)}
          </code>
          <span className="text-right text-xs tabular-nums text-muted-foreground">{a.rss_kb === null ? "n/a" : mb(a.rss_kb)}</span>
          <div aria-hidden className="col-span-2 h-1.5 overflow-hidden rounded-full bg-muted">
            <div className="h-full rounded-full bg-primary/70" style={{ width: `${((a.rss_kb ?? 0) / max) * 100}%` }} />
          </div>
        </li>
      ))}
    </ul>
  );
}

export function Server({ api }: { api: Api }) {
  const [s, error, reload] = useLoad<StatusResp>(() => api.status(), [api]);
  useEffect(() => {
    const t = setInterval(reload, 10_000);
    return () => clearInterval(t);
  }, [reload]);
  return (
    <Section
      id="server"
      title="Server"
      description="The cmuxd this app talks to. Updates every 10 seconds."
      actions={
        <Button variant="ghost" size="icon-sm" aria-label="Refresh" onClick={reload}>
          <RefreshCwIcon />
        </Button>
      }
    >
      <Err text={error} />
      {!s && !error && <Skeleton className="h-24 w-full" />}
      {s && (
        <>
          <KV
            rows={[
              ["Version", `${s.server.name} ${s.server.version}`],
              ["Uptime", uptime(s.uptime_secs)],
              ["Revision", s.rev],
              ["Load", s.load ? s.load.map((x) => x.toFixed(2)).join(" ") : "n/a"],
              ["Live processes", s.live_processes],
              ["Clients", s.clients],
              ["GitHub", s.github || "n/a"],
            ]}
          />
          <h4 className="mt-2 text-sm font-medium">Memory by agent</h4>
          <RssList agents={s.agents} />
        </>
      )}
    </Section>
  );
}

export function useSettings(api: Api) {
  return useLoad<SettingsResp>(() => api.settings(), [api]);
}

export function Limits({ settings: [s, error] }: { settings: ReturnType<typeof useSettings> }) {
  return (
    <Section id="limits" title="Limits" description="How much runs at once. Set these in config.toml on the server.">
      <Err text={error} />
      {!s && !error && <Skeleton className="h-24 w-full" />}
      {s && (
        <KV
          rows={[
            ["Live Claude processes", s.live_processes],
            ["Task agents per plan", s.tasks_per_plan],
            ["Sub-agents per task", s.subagents_per_task],
            ["Permission wait", `${Math.round(s.permission_wait_secs / 60)} min`],
          ]}
        />
      )}
    </Section>
  );
}

export function Claude({ settings: [s, error] }: { settings: ReturnType<typeof useSettings> }) {
  return (
    <Section id="claude" title="Claude" description="The model and permission mode per agent level. Set these in config.toml.">
      <Err text={error} />
      {!s && !error && <Skeleton className="h-24 w-full" />}
      {s && (
        <ItemGroup className="gap-2">
          {s.claude.map((l) => (
            <Item key={l.level} variant="outline" size="sm">
              <ItemContent>
                <ItemTitle className="capitalize">{l.level}</ItemTitle>
                <ItemDescription>{l.model ?? "Default model"}</ItemDescription>
              </ItemContent>
              <ItemActions>
                <Badge variant="secondary">{l.permission_mode}</Badge>
              </ItemActions>
            </Item>
          ))}
        </ItemGroup>
      )}
    </Section>
  );
}
