// The chat view's model (UI design 2.4): entries from the step-10 stream,
// merged by uuid, and folded into rows: your prompts, Claude's text, and one
// line per tool call with its result folded under it.

import type { ChatEntry } from "./api/types";

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
  | { kind: "user" | "assistant"; key: string; text: string }
  | {
      kind: "tool";
      key: string;
      name: string;
      summary: string;
      /** The folded output; `null` while the call runs. */
      result: string | null;
    };

/** One row per prompt, text and tool call. A result folds into its call. */
export function chatRows(entries: ChatEntry[]): ChatRow[] {
  const rows: ChatRow[] = [];
  for (const e of entries) {
    if (e.kind === "text") {
      rows.push({ kind: e.role, key: e.uuid, text: e.text });
    } else if (e.kind === "tool_call") {
      rows.push({
        kind: "tool",
        key: e.uuid,
        name: e.tool?.name ?? "tool",
        summary: e.tool?.summary ?? e.text,
        result: null,
      });
    } else {
      // The latest call of this tool still without a result.
      let call: ChatRow | undefined;
      for (let i = rows.length - 1; i >= 0; i--) {
        const r = rows[i];
        if (r.kind === "tool" && r.result === null && (!e.tool || r.name === e.tool.name)) {
          call = r;
          break;
        }
      }
      if (call && call.kind === "tool") call.result = e.text;
      else
        rows.push({
          kind: "tool",
          key: e.uuid,
          name: e.tool?.name ?? "tool",
          summary: e.tool?.summary ?? "",
          result: e.text,
        });
    }
  }
  return rows;
}
