import { useEffect, useState } from "preact/hooks";
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
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
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

/** Settings (UI design 4.3). */
export function Settings({ api, readOnly }: Props) {
  return (
    <section class="settings form-page">
      <h2>Settings</h2>
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
  const revoke = async (d: DeviceView) => {
    setBusy(d.id);
    try {
      await api.revokeDevice(d.id);
    } finally {
      setBusy(null);
      reload();
    }
  };
  return (
    <section>
      <h3>Devices</h3>
      {error && <p class="error small">{error}</p>}
      <table class="kv">
        <tbody>
          {resp?.devices.map((d) => (
            <tr key={d.id}>
              <td>
                {d.name}
                {d.current && <span class="muted small"> (this device)</span>}
                <div class="muted small">
                  {d.scope} · last seen {when(d.last_seen)}
                </div>
              </td>
              <td class="right">
                <button class="ghost" disabled={readOnly || busy === d.id} onClick={() => void revoke(d)}>
                  Revoke
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <p class="muted small">Pair a new device with <code>cmuxd pair</code> on the server.</p>
    </section>
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
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    pushState().then(setPush, () => setPush("unsupported"));
  }, []);
  const run = async (f: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
      setPush(await pushState().catch(() => "unsupported" as const));
    }
  };
  const flip = (k: string) => {
    const next = { ...on, [k]: !on[k] };
    setOn(next);
    saveNotify(next);
    if (push === "on") void run(() => updateKinds(api, kindsOf(next)));
  };
  return (
    <section>
      <h3>Notifications</h3>
      <p class="muted small">What notifies this device.</p>
      {NOTIFY_KINDS.map(([k, label]) => (
        <label key={k} class="check">
          <input type="checkbox" checked={on[k] !== false} onChange={() => flip(k)} /> {label}
        </label>
      ))}
      {push && <p class="muted small">{PUSH_TEXT[push]}</p>}
      {push === "off" && (
        <button disabled={busy} onClick={() => void run(() => enablePush(api, kindsOf(on)))}>
          Enable push
        </button>
      )}
      {push === "on" && (
        <button class="ghost" disabled={busy} onClick={() => void run(() => disablePush(api))}>
          Disable
        </button>
      )}
      {error && <p class="error small">{error}</p>}
    </section>
  );
}

function Install() {
  const install = useInstall();
  if (isStandalone()) return null;
  if (install)
    return (
      <section>
        <h3>App</h3>
        <button onClick={() => void install()}>Install app</button>
      </section>
    );
  if (isIos())
    return (
      <section>
        <h3>App</h3>
        <p class="muted small">Install: Share → Add to Home Screen.</p>
      </section>
    );
  return null;
}

function Repositories({ api, readOnly }: Props) {
  const { repos, error, add } = useRepos(api);
  return (
    <section>
      <h3>Repositories</h3>
      {error && <p class="error small">{error}</p>}
      <ul class="plain">
        {repos?.map((r) => (
          <li key={r.path}>
            <code>{r.path}</code> <span class="muted small">{r.base_branch}</span>
            {r.remote_url && <div class="muted small">{r.remote_url}</div>}
          </li>
        ))}
      </ul>
      {repos?.length === 0 && <p class="muted small">None yet.</p>}
      <AddRepo add={add} disabled={readOnly} />
    </section>
  );
}

function ServerSettings({ api }: { api: Api }) {
  const [s, error] = useLoad<SettingsResp>(() => api.settings(), [api]);
  return (
    <>
      <section>
        <h3>Limits</h3>
        {error && <p class="error small">{error}</p>}
        {s && (
          <table class="kv">
            <tbody>
              <tr>
                <td>Live Claude processes</td>
                <td class="right">{s.live_processes}</td>
              </tr>
              <tr>
                <td>Task agents per plan</td>
                <td class="right">{s.tasks_per_plan}</td>
              </tr>
              <tr>
                <td>Sub-agents per task</td>
                <td class="right">{s.subagents_per_task}</td>
              </tr>
              <tr>
                <td>Permission wait</td>
                <td class="right">{Math.round(s.permission_wait_secs / 60)} min</td>
              </tr>
            </tbody>
          </table>
        )}
      </section>
      <section>
        <h3>Claude</h3>
        {s && (
          <table class="kv">
            <thead>
              <tr>
                <th>Level</th>
                <th>Model</th>
                <th>Permission mode</th>
              </tr>
            </thead>
            <tbody>
              {s.claude.map((l) => (
                <tr key={l.level}>
                  <td>{l.level}</td>
                  <td>{l.model ?? <span class="muted">default</span>}</td>
                  <td>{l.permission_mode}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <p class="muted small">
          Set these in <code>config.toml</code> on the server.
        </p>
      </section>
    </>
  );
}

function Status({ api }: { api: Api }) {
  const [s, error, reload] = useLoad<StatusResp>(() => api.status(), [api]);
  useEffect(() => {
    const t = setInterval(reload, 10_000);
    return () => clearInterval(t);
  }, []);
  return (
    <section>
      <h3>Server status</h3>
      {error && <p class="error small">{error}</p>}
      {s && (
        <table class="kv">
          <tbody>
            <tr>
              <td>Version</td>
              <td class="right">
                {s.server.name} {s.server.version}
              </td>
            </tr>
            <tr>
              <td>Uptime</td>
              <td class="right">{uptime(s.uptime_secs)}</td>
            </tr>
            <tr>
              <td>Load</td>
              <td class="right">{s.load ? s.load.map((x) => x.toFixed(2)).join(" ") : "n/a"}</td>
            </tr>
            <tr>
              <td>Clients · live processes</td>
              <td class="right">
                {s.clients} · {s.live_processes}
              </td>
            </tr>
            <tr>
              <td>GitHub</td>
              <td class="right">{s.github || "n/a"}</td>
            </tr>
            {s.agents.map((a) => (
              <tr key={a.agent}>
                <td>
                  <code>{a.agent.slice(a.agent.indexOf("/") + 1)}</code>
                </td>
                <td class="right">{a.rss_kb === null ? "n/a" : `${Math.round(a.rss_kb / 1024)} MB`}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
