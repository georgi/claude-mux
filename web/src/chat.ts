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
      /** The model that wrote an assistant text, when known. */
      model?: string;
    }
  | { kind: "thinking"; key: string; text: string }
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
    };

type ToolRow = Extract<ChatRow, { kind: "tool" }>;

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
  for (const e of entries) {
    if (e.kind === "text") {
      rows.push({ kind: e.role, key: e.uuid, text: e.text, ...(e.model ? { model: e.model } : {}) });
    } else if (e.kind === "thinking") {
      if (opts.thinking) rows.push({ kind: "thinking", key: e.uuid, text: e.text });
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
      });
    } else {
      const result = {
        result: e.text,
        isError: e.is_error ?? false,
        resultKey: e.uuid,
        resultLen: e.full_len,
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
