// The chat view's model (UI design 2.4): entries from the step-10 stream,
// merged by uuid, and folded into rows: your prompts, Claude's text and
// thinking, and one line per tool call with its result folded under it.

import type { ChatEntry, JsonValue } from "./api/types";

/** Entries kept per agent; older ones fall off the top. */
export const KEEP = 2000;

/**
 * `incoming` after `have`, without repeats. A resubscribe after a reconnect or
 * a lagging tail can send an entry twice; its uuid is the same.
 */
export function mergeEntries(have: ChatEntry[], incoming: ChatEntry[]): ChatEntry[] {
  if (incoming.length === 0) return have;
  const seen = new Set(have.map((e) => e.uuid));
  const fresh = incoming.filter((e) => !seen.has(e.uuid) && (seen.add(e.uuid), true));
  if (fresh.length === 0) return have;
  const out = have.concat(fresh);
  return out.length > KEEP ? out.slice(out.length - KEEP) : out;
}

export type ChatRow =
  | {
      kind: "user" | "assistant";
      key: string;
      text: string;
      /** When the entry arrived, in ms. */
      ts: number;
      /** The model that wrote an assistant text, when known. */
      model?: string;
    }
  | {
      kind: "thinking";
      key: string;
      text: string;
      ts: number;
      /** Until the next entry, in ms; absent while the thinking is the latest entry. */
      ms?: number;
    }
  | {
      kind: "tool";
      key: string;
      name: string;
      summary: string;
      /** The folded output; `null` while the call runs. */
      result: string | null;
      /** The `tool_use` id that pairs the call with its result. */
      callId?: string;
      /** The structured input of a known tool (`ChatTool.input`). */
      input?: JsonValue;
      /** The result is an error. */
      isError: boolean;
      /** The result entry's uuid, for `api.entry` when `resultLen` says it was cut. */
      resultKey?: string;
      /** The uncut result's length in chars, when `result` was cut. */
      resultLen?: number;
      /** When the call (or a lone result) arrived, in ms. */
      ts: number;
      /** When the result arrived, in ms. */
      endTs?: number;
    };

export type ToolRow = Extract<ChatRow, { kind: "tool" }>;

export interface RowOptions {
  /** Keep thinking entries as rows; they are dropped otherwise. */
  thinking?: boolean;
}

/**
 * The call a result answers: the call with its `call_id`, else (an older
 * server, or the call fell off the top) the latest call of the same tool
 * that has no id and no result yet.
 */
function callOf(rows: ChatRow[], e: ChatEntry): ToolRow | undefined {
  if (e.call_id) {
    for (let i = rows.length - 1; i >= 0; i--) {
      const r = rows[i];
      if (r.kind === "tool" && r.callId === e.call_id) return r.result === null ? r : undefined;
    }
  }
  for (let i = rows.length - 1; i >= 0; i--) {
    const r = rows[i];
    // A call with another id is never this result's.
    if (r.kind !== "tool" || r.result !== null || (e.call_id && r.callId)) continue;
    if (!e.tool || r.name === e.tool.name) return r;
  }
  return undefined;
}

/** One row per prompt, text and tool call. A result folds into its call. */
export function chatRows(entries: ChatEntry[], opts: RowOptions = {}): ChatRow[] {
  const rows: ChatRow[] = [];
  // The thinking that waits for the next entry to know how long it took.
  let thought: Extract<ChatRow, { kind: "thinking" }> | undefined;
  for (const e of entries) {
    if (thought) {
      thought.ms = Math.max(0, e.ts - thought.ts);
      thought = undefined;
    }
    if (e.kind === "text") {
      rows.push({ kind: e.role, key: e.uuid, text: e.text, ts: e.ts, ...(e.model ? { model: e.model } : {}) });
    } else if (e.kind === "thinking") {
      if (opts.thinking) rows.push((thought = { kind: "thinking", key: e.uuid, text: e.text, ts: e.ts }));
    } else if (e.kind === "tool_call") {
      rows.push({
        kind: "tool",
        key: e.uuid,
        name: e.tool?.name ?? "tool",
        summary: e.tool?.summary ?? e.text,
        result: null,
        callId: e.call_id,
        input: e.tool?.input,
        isError: false,
        ts: e.ts,
      });
    } else {
      const result = {
        result: e.text,
        isError: e.is_error ?? false,
        resultKey: e.uuid,
        resultLen: e.full_len,
        endTs: e.ts,
      };
      const call = callOf(rows, e);
      if (call) Object.assign(call, result);
      else
        rows.push({
          kind: "tool",
          key: e.uuid,
          name: e.tool?.name ?? "tool",
          summary: e.tool?.summary ?? "",
          callId: e.call_id,
          ts: e.ts,
          ...result,
        });
    }
  }
  return rows;
}

export type ToolKind = "shell" | "file" | "read" | "search" | "agent" | "web" | "other";

/** The icon family of a tool, by its name (`Bash`, `Edit`, `mcp__x__y`, ...). */
export function toolKind(name: string): ToolKind {
  switch (name.toLowerCase()) {
    case "bash":
    case "bashoutput":
    case "killshell":
      return "shell";
    case "edit":
    case "multiedit":
    case "write":
    case "notebookedit":
      return "file";
    case "read":
      return "read";
    case "grep":
    case "glob":
    case "ls":
      return "search";
    case "task":
    case "agent":
      return "agent";
    case "webfetch":
    case "websearch":
      return "web";
    default:
      return "other";
  }
}

/** Consecutive tool calls folded under one "Worked for N steps" line. */
export interface ToolGroup {
  kind: "group";
  /** The first row's key. */
  key: string;
  /** The tool rows, and any thinking between them. */
  rows: ChatRow[];
  /** Tool calls in the group. */
  steps: number;
  /** A call in the group has no result yet. */
  running: boolean;
  /** From the first call to the last result (or the last call while running), in ms. */
  ms: number;
}

export type LogItem = ChatRow | ToolGroup;

/** Runs of fewer tool calls than this stay as single rows. */
export const GROUP_MIN = 3;

/**
 * Folds each run of at least `min` tool rows with no text between them into
 * one group. Thinking rows do not break a run; prompts and replies do.
 */
export function groupRows(rows: ChatRow[], min = GROUP_MIN): LogItem[] {
  const out: LogItem[] = [];
  let run: ChatRow[] = [];
  const flush = () => {
    const tools = run.filter((r): r is ToolRow => r.kind === "tool");
    if (tools.length >= min) {
      const first = tools[0];
      const last = tools[tools.length - 1];
      const running = tools.some((t) => t.result === null);
      const end = Math.max(...tools.map((t) => t.endTs ?? t.ts));
      const ms = Math.max(0, (running ? last.ts : end) - first.ts);
      out.push({ kind: "group", key: `group-${run[0].key}`, rows: run, steps: tools.length, running, ms });
    } else out.push(...run);
    run = [];
  };
  for (const r of rows) {
    if (r.kind === "tool" || r.kind === "thinking") run.push(r);
    else {
      flush();
      out.push(r);
    }
  }
  flush();
  return out;
}

/** "45s", "1m 12s", "2h 5m". */
export function formatDuration(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

/** The key of the last tool row named `name`, for example the latest `TodoWrite`. */
export function lastToolKey(rows: ChatRow[], name: string): string | undefined {
  for (let i = rows.length - 1; i >= 0; i--) {
    const r = rows[i];
    if (r.kind === "tool" && r.name === name) return r.key;
  }
  return undefined;
}

/** A run of the log under one header: a prompt, or everything Claude did until the next prompt. */
export interface Turn {
  kind: "user" | "assistant";
  /** The first item's key. */
  key: string;
  items: LogItem[];
  /** When the turn started, in ms. */
  ts: number;
  /** The model of the turn's first reply, when known. */
  model?: string;
  /** The turn's text: the prompt, or Claude's replies joined. */
  text: string;
  /**
   * Changes when anything shown in the turn changes. Rows are rebuilt on each
   * batch, so the view compares this, not the objects.
   */
  sig: string;
}

function itemTs(i: LogItem): number {
  return i.kind === "group" ? i.rows[0].ts : i.ts;
}

function rowSig(r: ChatRow): string {
  if (r.kind === "tool") return `${r.key}:${r.result === null ? "-" : r.result.length}:${r.isError ? 1 : 0}:${r.resultLen ?? ""}`;
  if (r.kind === "thinking") return `${r.key}:${r.text.length}:${r.ms ?? ""}`;
  return `${r.key}:${r.text.length}`;
}

/** Folds log items into turns: each prompt alone, and each run of Claude's items together. */
export function turnsOf(items: LogItem[]): Turn[] {
  const out: Turn[] = [];
  let cur: Turn | undefined;
  for (const i of items) {
    if (i.kind === "user") {
      cur = undefined;
      out.push({ kind: "user", key: i.key, items: [i], ts: i.ts, text: i.text, sig: rowSig(i) });
      continue;
    }
    if (!cur) {
      cur = { kind: "assistant", key: i.key, items: [], ts: itemTs(i), text: "", sig: "" };
      out.push(cur);
    }
    cur.items.push(i);
    if (i.kind === "assistant") {
      cur.text = cur.text ? `${cur.text}\n\n${i.text}` : i.text;
      cur.model ??= i.model;
    }
    cur.sig += `|${i.kind === "group" ? i.rows.map(rowSig).join(",") : rowSig(i)}`;
  }
  return out;
}

const dayFmt = new Intl.DateTimeFormat("en-US", { weekday: "short", month: "short", day: "numeric" });
const dayYearFmt = new Intl.DateTimeFormat("en-US", { weekday: "short", month: "short", day: "numeric", year: "numeric" });

/** The local calendar day of `ts`, as a key: two times on one day share it. */
export function dayKey(ts: number): string {
  const d = new Date(ts);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

/** "Today", "Yesterday", "Mon, Oct 19", or with the year when it differs from now. */
export function dayLabel(ts: number, now: number): string {
  if (dayKey(ts) === dayKey(now)) return "Today";
  const y = new Date(now);
  y.setDate(y.getDate() - 1);
  if (dayKey(ts) === dayKey(y.getTime())) return "Yesterday";
  return (new Date(ts).getFullYear() === new Date(now).getFullYear() ? dayFmt : dayYearFmt).format(ts);
}

/** Markdown as plain text, for "Copy text": no markup characters, links as their words. */
export function plainText(md: string): string {
  return md
    .replace(/^```[^\n]*\n?/gm, "")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^#{1,6}\s+/gm, "")
    .replace(/^\s*>\s?/gm, "")
    .replace(/(\*\*|__)(.+?)\1/g, "$2")
    .replace(/(^|[^*\w])[*_]([^*_\n]+)[*_]/g, "$1$2")
    .replace(/`([^`]+)`/g, "$1")
    .trim();
}

/** `text` as a Markdown quote, ready for the composer. */
export function quoted(text: string): string {
  return `${text
    .trim()
    .split("\n")
    .map((l) => (l ? `> ${l}` : ">"))
    .join("\n")}\n\n`;
}
