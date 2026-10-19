import { BellIcon, DownloadIcon, FolderGitIcon, GaugeIcon, ServerIcon, SmartphoneIcon, TrashIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import type { Api } from "../api/http";
import type { DeviceView, SettingsResp, StatusResp } from "../api/types";
import {
  disablePush,
  enablePush,
  kindsOf,
  loadNotify,
  NOTIFY_KINDS,
  type PushState,
  pushState,
  saveNotify,
  updateKinds,
} from "../push";
import { isIos, isStandalone, useInstall } from "../pwa";
import { AddRepo, useRepos } from "./Board";
import { errText } from "./board/state";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from "@/components/ui/item";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";

interface Props {
  api: Api;
  readOnly?: boolean;
}

function useLoad<T>(f: () => Promise<T>, deps: unknown[]): [T | null, string | null, () => void] {
  const [v, setV] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    f()
      .then((x) => {
        setV(x);
        setError(null);
      })
      .catch((e: unknown) => setError(errText(e)));
    // biome-ignore lint/correctness/useExhaustiveDependencies: f changes each render; deps drive reloads.
  }, [...deps, n]);
  return [v, error, () => setN((x) => x + 1)];
}

function when(ms: number | null): string {
  return ms ? new Date(ms).toLocaleString() : "never";
}

function uptime(s: number): string {
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`;
}

function Err({ text }: { text: string | null }) {
  return text ? <p className="text-sm text-destructive">{text}</p> : null;
}

function Section({ icon, title, description, children }: { icon: React.ReactNode; title: string; description?: string; children: React.ReactNode }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <span aria-hidden className="text-muted-foreground [&_svg]:size-4">
            {icon}
          </span>
          {title}
        </CardTitle>
        {description && <CardDescription>{description}</CardDescription>}
      </CardHeader>
      <CardContent className="flex flex-col gap-3">{children}</CardContent>
    </Card>
  );
}

function KV({ rows }: { rows: [React.ReactNode, React.ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[1fr_auto] gap-x-4 gap-y-2 text-sm">
      {rows.map(([k, v], i) => (
        <div key={i} className="col-span-2 grid grid-cols-subgrid items-baseline border-b border-border/50 pb-2 last:border-0 last:pb-0">
          <dt className="min-w-0 text-muted-foreground">{k}</dt>
          <dd className="text-right tabular-nums">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

/** Settings (UI design 4.3). */
export function Settings({ api, readOnly }: Props) {
  return (
    <section className="mx-auto flex w-full max-w-2xl flex-col gap-4 p-4">
      <h2 className="text-lg font-medium">Settings</h2>
      <Devices api={api} readOnly={readOnly} />
      <Notifications api={api} />
      <Install />
      <Repositories api={api} readOnly={readOnly} />
      <ServerSettings api={api} />
      <Status api={api} />
    </section>
  );
}

function Devices({ api, readOnly }: Props) {
  const [resp, error, reload] = useLoad(() => api.devices(), [api]);
  const [busy, setBusy] = useState<string | null>(null);
  const [target, setTarget] = useState<DeviceView | null>(null);
  const revoke = async (d: DeviceView) => {
    setBusy(d.id);
    try {
      await api.revokeDevice(d.id);
      toast.success(`Revoked ${d.name}`);
    } catch (e) {
      toast.error(errText(e));
    } finally {
      setBusy(null);
      setTarget(null);
      reload();
    }
  };
  return (
    <Section icon={<SmartphoneIcon />} title="Devices" description="Paired with cmuxd pair on the server.">
      <Err text={error} />
      {!resp && !error && <Skeleton className="h-12 w-full" />}
      <ItemGroup className="gap-2">
        {resp?.devices.map((d) => (
          <Item key={d.id} variant="outline" size="sm">
            <ItemContent>
              <ItemTitle>
                {d.name}
                {d.current && <Badge variant="secondary">this device</Badge>}
              </ItemTitle>
              <ItemDescription>
                {d.scope} · last seen {when(d.last_seen)}
              </ItemDescription>
            </ItemContent>
            <ItemActions>
              <Button variant="destructive" size="sm" disabled={readOnly || busy === d.id} onClick={() => setTarget(d)} aria-label={`Revoke ${d.name}`}>
                <TrashIcon /> Revoke
              </Button>
            </ItemActions>
          </Item>
        ))}
      </ItemGroup>
      <Dialog open={target !== null} onOpenChange={(o) => !o && setTarget(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Revoke {target?.name}?</DialogTitle>
            <DialogDescription>
              {target?.current ? "This is the device you use now. You will lose access. " : ""}The device must pair again to connect.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <DialogClose render={<Button variant="outline" />}>Cancel</DialogClose>
            <Button variant="destructive" disabled={!target || busy !== null} onClick={() => target && void revoke(target)}>
              Revoke device
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </Section>
  );
}

const PUSH_TEXT: Record<PushState, string> = {
  unsupported: "This browser has no Web Push. On iOS, add the app to the home screen first.",
  insecure: "Web Push needs HTTPS.",
  denied: "Notifications are blocked for this site in the browser settings.",
  off: "Push is off on this device.",
  on: "Push is on for this device.",
};

function Notifications({ api }: { api: Api }) {
  const [on, setOn] = useState(loadNotify);
  const [push, setPush] = useState<PushState | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    pushState().then(setPush, () => setPush("unsupported"));
  }, []);
  const run = async (f: () => Promise<unknown>, done?: string) => {
    setBusy(true);
    try {
      await f();
      if (done) toast.success(done);
    } catch (e) {
      toast.error(errText(e));
    } finally {
      setBusy(false);
      setPush(await pushState().catch(() => "unsupported" as const));
    }
  };
  const flip = (k: string) => {
    const next = { ...on, [k]: on[k] === false };
    setOn(next);
    saveNotify(next);
    if (push === "on") void run(() => updateKinds(api, kindsOf(next)));
  };
  return (
    <Section icon={<BellIcon />} title="Notifications" description="What notifies this device.">
      <div className="flex flex-col gap-3">
        {NOTIFY_KINDS.map(([k, label]) => (
          <div key={k} className="flex items-center justify-between gap-3">
            <Label htmlFor={`notify-${k}`}>{label}</Label>
            <Switch id={`notify-${k}`} checked={on[k] !== false} onCheckedChange={() => flip(k)} />
          </div>
        ))}
      </div>
      {push && <p className="text-sm text-muted-foreground">{PUSH_TEXT[push]}</p>}
      <div>
        {push === "off" && (
          <Button disabled={busy} onClick={() => void run(() => enablePush(api, kindsOf(on)), "Push enabled")}>
            Enable push
          </Button>
        )}
        {push === "on" && (
          <Button variant="outline" disabled={busy} onClick={() => void run(() => disablePush(api), "Push disabled")}>
            Disable push
          </Button>
        )}
      </div>
    </Section>
  );
}

function Install() {
  const install = useInstall();
  if (isStandalone()) return null;
  if (install)
    return (
      <Section icon={<DownloadIcon />} title="App">
        <div>
          <Button onClick={() => void install()}>Install app</Button>
        </div>
      </Section>
    );
  if (isIos())
    return (
      <Section icon={<DownloadIcon />} title="App">
        <p className="text-sm text-muted-foreground">Install: Share → Add to Home Screen.</p>
      </Section>
    );
  return null;
}

function Repositories({ api, readOnly }: Props) {
  const { repos, error, add } = useRepos(api);
  return (
    <Section icon={<FolderGitIcon />} title="Repositories">
      <Err text={error} />
      {repos?.length === 0 && <p className="text-sm text-muted-foreground">None yet.</p>}
      <ItemGroup className="gap-2">
        {repos?.map((r) => (
          <Item key={r.path} variant="outline" size="sm">
            <ItemContent>
              <ItemTitle className="font-mono text-xs break-all">{r.path}</ItemTitle>
              {r.remote_url && <ItemDescription className="break-all">{r.remote_url}</ItemDescription>}
            </ItemContent>
            <ItemActions>
              <Badge variant="outline">{r.base_branch}</Badge>
            </ItemActions>
          </Item>
        ))}
      </ItemGroup>
      <AddRepo add={add} disabled={readOnly} />
    </Section>
  );
}

function ServerSettings({ api }: { api: Api }) {
  const [s, error] = useLoad<SettingsResp>(() => api.settings(), [api]);
  return (
    <>
      <Section icon={<GaugeIcon />} title="Limits">
        <Err text={error} />
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
      <Section icon={<ServerIcon />} title="Claude" description="Set these in config.toml on the server.">
        {s && (
          <ItemGroup className="gap-2">
            {s.claude.map((l) => (
              <Item key={l.level} variant="outline" size="sm">
                <ItemContent>
                  <ItemTitle>{l.level}</ItemTitle>
                  <ItemDescription>{l.model ?? "default model"}</ItemDescription>
                </ItemContent>
                <ItemActions>
                  <Badge variant="secondary">{l.permission_mode}</Badge>
                </ItemActions>
              </Item>
            ))}
          </ItemGroup>
        )}
      </Section>
    </>
  );
}

function Status({ api }: { api: Api }) {
  const [s, error, reload] = useLoad<StatusResp>(() => api.status(), [api]);
  useEffect(() => {
    const t = setInterval(reload, 10_000);
    return () => clearInterval(t);
    // biome-ignore lint/correctness/useExhaustiveDependencies: reload is stable enough; one interval per mount.
  }, []);
  return (
    <Section icon={<ServerIcon />} title="About and server status">
      <Err text={error} />
      {!s && !error && <Skeleton className="h-24 w-full" />}
      {s && (
        <KV
          rows={[
            ["Version", `${s.server.name} ${s.server.version}`],
            ["Uptime", uptime(s.uptime_secs)],
            ["Load", s.load ? s.load.map((x) => x.toFixed(2)).join(" ") : "n/a"],
            ["Clients · live processes", `${s.clients} · ${s.live_processes}`],
            ["GitHub", s.github || "n/a"],
            ...s.agents.map((a): [React.ReactNode, React.ReactNode] => [
              <code key={a.agent}>{a.agent.slice(a.agent.indexOf("/") + 1)}</code>,
              a.rss_kb === null ? "n/a" : `${Math.round(a.rss_kb / 1024)} MB`,
            ]),
          ]}
        />
      )}
    </Section>
  );
}
