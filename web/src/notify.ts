// What the service worker shows for a push and does on a tap. Pure, so the
// tests check it without a worker; `sw.ts` is the only other importer.

import type { AnswerReq, NotifyKind, PushPayload } from "./api/types";

const TITLES: Record<NotifyKind, string> = {
  permission: "Permission request",
  question: "Question",
  plan_review: "Plan ready for review",
  stalled: "Agent stalled",
  ci_failing: "CI failing",
  plan_done: "Plan finished",
};

export interface Action {
  action: string;
  title: string;
  /** Chrome's inline reply: the text comes back as `event.reply`. */
  type?: "text";
  placeholder?: string;
}

export interface Shown {
  title: string;
  options: { body: string; tag: string; data: PushPayload; icon: string; badge: string; actions: Action[] };
}

export function parsePayload(text: string): PushPayload | null {
  try {
    const p = JSON.parse(text) as PushPayload;
    return p && typeof p.kind === "string" ? p : null;
  } catch {
    return null;
  }
}

export function notification(p: PushPayload): Shown {
  const actions: Action[] =
    p.item === null
      ? []
      : p.kind === "permission"
        ? [
            { action: "allow", title: "Allow" },
            { action: "deny", title: "Deny" },
          ]
        : p.kind === "question"
          ? [{ action: "reply", title: "Reply", type: "text", placeholder: "Answer" }]
          : [];
  // One notification per item: a later push for it replaces the earlier one.
  const tag = p.item !== null ? `item-${p.item}` : p.plan ? `plan-${p.plan}` : p.kind;
  return {
    title: TITLES[p.kind] ?? "cmux",
    options: { body: p.summary, tag, data: p, icon: "/icon-192.png", badge: "/icon-192.png", actions },
  };
}

/** The answer a tap on an action sends, or null when the tap opens the app. */
export function answerFor(p: PushPayload, action: string, reply: string | null): AnswerReq | null {
  if (p.item === null) return null;
  if (action === "allow" || action === "deny") return { action, text: null };
  if (action === "reply" && reply) return { action: "reply", text: reply };
  return null;
}

/** Where a tap that opens the app lands (see route.ts). */
export function clickUrl(p: PushPayload): string {
  return p.kind === "plan_done" && p.plan ? `/#/board/${encodeURIComponent(p.plan)}` : "/#/needs";
}
