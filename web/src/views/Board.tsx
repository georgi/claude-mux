import { useEffect, useState } from "preact/hooks";
import type { Api } from "../api/http";
import type { PlanView, RepoInfo, StateView, TaskView } from "../api/types";
import { useSpinner } from "../hooks";
import { agentGlyph } from "../tree";

interface BoardProps {
  view: StateView;
  plan: string;
  api: Api;
  readOnly?: boolean;
  onOpenAgent: (agent: string) => void;
}

function useAction(): [boolean, string | null, (f: () => Promise<unknown>) => void] {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = (f: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    f()
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false));
  };
  return [busy, error, run];
}

function waits(t: TaskView, tasks: TaskView[]): string {
  const open = t.deps.filter((d) => tasks.find((x) => x.id === d)?.state !== "merged");
  return open.length ? `waits ${open.join(", ")}` : "";
}

/** One plan's tasks as a table (UI design 3.6), with the plan's controls. */
export function Board({ view, plan, api, readOnly, onOpenAgent }: BoardProps) {
  const p = view.plans.find((x) => x.id === plan);
  const spin = useSpinner(view.agents.some((a) => a.activity === "working"));
  const [busy, error, run] = useAction();
  if (!p) return <p class="muted pad">No plan {plan}.</p>;
  const tasks = p.tasks.map((id) => view.tasks.find((t) => t.plan === p.id && t.id === id)).filter((t): t is TaskView => !!t);
  const executor = view.agents.find((a) => a.id === `${p.id}/executor`);
  const can = !readOnly && !busy;
  return (
    <section class="board">
      <header class="board-head">
        <h2>{p.title}</h2>
        <span class={`badge ${p.state}`}>{p.paused ? "paused" : p.state}</span>
        <span class="grow" />
        {(p.state === "draft" || p.state === "proposed") && (
          <button disabled={!can} onClick={() => run(() => api.plan(p.id, "accept"))}>
            Accept
          </button>
        )}
        {p.state === "accepted" &&
          (p.paused ? (
            <button disabled={!can} onClick={() => run(() => api.plan(p.id, "resume"))}>
              Resume
            </button>
          ) : (
            <button class="ghost" disabled={!can} onClick={() => run(() => api.plan(p.id, "pause"))}>
              Pause
            </button>
          ))}
      </header>
      {error && <p class="error small pad">{error}</p>}
      <p class="muted small board-sub">
        {p.id} · {p.repos.map((r) => r.path).join(", ")}
      </p>
      {executor && (
        <button class="row-button" onClick={() => onOpenAgent(executor.id)}>
          <span class="glyph">◆{agentGlyph(executor, spin)}</span> executor
        </button>
      )}
      {tasks.length === 0 ? (
        <p class="muted pad">No tasks yet.</p>
      ) : (
        <table class="board-table">
          <thead>
            <tr>
              <th />
              <th>Task</th>
              <th>State</th>
              <th>Criteria</th>
              <th>PR</th>
            </tr>
          </thead>
          <tbody>
            {tasks.map((t) => {
              const a = t.agent ? view.agents.find((x) => x.id === t.agent) : undefined;
              const subs = a ? view.agents.filter((x) => x.parent === a.id) : [];
              return (
                <tr
                  key={t.id}
                  class={a ? "open" : ""}
                  onClick={a ? () => onOpenAgent(a.id) : undefined}
                  tabIndex={a ? 0 : -1}
                  onKeyDown={a ? (e) => e.key === "Enter" && onOpenAgent(a.id) : undefined}
                >
                  <td class="glyph">{a ? agentGlyph(a, spin) : "·"}</td>
                  <td>
                    <code>{t.id}</code> {t.title}
                    {subs.length > 0 && (
                      <div class="muted small">
                        {subs.map((s) => `${agentGlyph(s, spin)} ${s.path.slice(s.path.indexOf("/") + 1)}`).join("  ")}
                      </div>
                    )}
                    {t.state === "todo" && waits(t, tasks) && <div class="muted small">{waits(t, tasks)}</div>}
                  </td>
                  <td>
                    <span class={`badge ${t.state}`}>{t.state.replace("_", " ")}</span>
                  </td>
                  <td class="muted">
                    {t.criteria_done}/{t.criteria_total}
                  </td>
                  <td>
                    {t.pr ? (
                      <a href={t.pr.url} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()}>
                        #{t.pr.number}
                        {t.pr.ci === "failing" ? " ✕" : t.pr.ci === "passing" ? " ✓" : ""}
                      </a>
                    ) : (
                      ""
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}

/** The phone's Plans tab: every plan, newest first. */
export function PlanList({ view, onOpen, onNew }: { view: StateView; onOpen: (plan: string) => void; onNew: () => void }) {
  const plans = [...view.plans].sort((a, b) => (a.id < b.id ? 1 : -1));
  const progress = (p: PlanView) => {
    const ts = view.tasks.filter((t) => t.plan === p.id);
    return `${ts.filter((t) => t.state === "merged").length}/${ts.length} merged`;
  };
  return (
    <section class="plan-list">
      <ul>
        {plans.map((p) => (
          <li key={p.id}>
            <button class="row-button" onClick={() => onOpen(p.id)}>
              <span class="label">{p.title}</span>
              <span class="muted small">{progress(p)}</span>
              <span class={`badge ${p.state}`}>{p.paused ? "paused" : p.state}</span>
            </button>
          </li>
        ))}
      </ul>
      {plans.length === 0 && <p class="muted pad">No plans yet.</p>}
      <div class="pad">
        <button onClick={onNew}>New plan</button>
      </div>
    </section>
  );
}

/**
 * The repositories the server knows, with a way to add one: a `git clone`
 * URL, or a path on the server.
 */
export function useRepos(api: Api): {
  repos: RepoInfo[] | null;
  error: string | null;
  add: (spec: string) => Promise<RepoInfo>;
} {
  const [repos, setRepos] = useState<RepoInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api
      .repos()
      .then((r) => setRepos(r.repos))
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
  }, [api]);
  const add = async (spec: string) => {
    const s = spec.trim();
    const r = await api.addRepo(s.startsWith("/") ? { path: s } : { url: s });
    setRepos((list) => [...(list ?? []).filter((x) => x.path !== r.path), r].sort((a, b) => (a.path < b.path ? -1 : 1)));
    return r;
  };
  return { repos, error, add };
}

export function AddRepo({ add, disabled }: { add: (spec: string) => Promise<RepoInfo>; disabled?: boolean }) {
  const [spec, setSpec] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const submit = async (e: Event) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await add(spec);
      setSpec("");
    } catch (x) {
      setError(x instanceof Error ? x.message : String(x));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form class="inline-form" onSubmit={submit}>
      <input
        aria-label="Repository URL or path"
        placeholder="git URL to clone, or /path on the server"
        value={spec}
        disabled={disabled}
        onInput={(e) => setSpec((e.target as HTMLInputElement).value)}
      />
      <button type="submit" disabled={disabled || busy || !spec.trim()}>
        {busy ? "Adding…" : "Add"}
      </button>
      {error && <p class="error small">{error}</p>}
    </form>
  );
}

/** New plan (UI design 4.2): a title, repositories, a first message for the executor. */
export function NewPlan({ api, readOnly, onCreated }: { api: Api; readOnly?: boolean; onCreated: (plan: string) => void }) {
  const { repos, error: repoError, add } = useRepos(api);
  const [title, setTitle] = useState("");
  const [picked, setPicked] = useState<string[]>([]);
  const [request, setRequest] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const toggle = (path: string) => setPicked((p) => (p.includes(path) ? p.filter((x) => x !== path) : [...p, path]));

  const submit = async (e: Event) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const r = await api.createPlan({
        title: title.trim(),
        repos: picked.map((path) => ({ path, base_branch: null })),
        ...(request.trim() ? { request: request.trim() } : {}),
      });
      onCreated(r.id);
    } catch (x) {
      setError(x instanceof Error ? x.message : String(x));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section class="form-page">
      <h2>New plan</h2>
      <form onSubmit={submit} class="stack">
        <label>
          Title
          <input value={title} onInput={(e) => setTitle((e.target as HTMLInputElement).value)} required />
        </label>
        <fieldset>
          <legend>Repositories</legend>
          {repoError && <p class="error small">{repoError}</p>}
          {repos === null && !repoError && <p class="muted small">Loading…</p>}
          {repos?.length === 0 && <p class="muted small">The server knows no repositories yet. Add one below.</p>}
          {repos?.map((r) => (
            <label key={r.path} class="check">
              <input type="checkbox" checked={picked.includes(r.path)} onChange={() => toggle(r.path)} />
              <code>{r.path}</code>
              {picked[0] === r.path && <span class="muted small"> (executor runs here)</span>}
            </label>
          ))}
        </fieldset>
        <AddRepo
          disabled={readOnly}
          add={async (s) => {
            const r = await add(s);
            setPicked((p) => (p.includes(r.path) ? p : [...p, r.path]));
            return r;
          }}
        />
        <label>
          First message for the executor
          <textarea
            rows={4}
            value={request}
            placeholder="What should this plan do?"
            onInput={(e) => setRequest((e.target as HTMLTextAreaElement).value)}
          />
        </label>
        <div class="actions">
          <button type="submit" disabled={readOnly || busy || !title.trim() || picked.length === 0}>
            {busy ? "Creating…" : "Create plan"}
          </button>
        </div>
        {error && <p class="error small">{error}</p>}
      </form>
    </section>
  );
}
