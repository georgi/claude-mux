// The needs-you list (UI design 2.3): the pending items addressed to a
// person, each with its one-tap actions. `actionsFor` decides which actions
// an item offers; `perform` turns one into REST calls. Both are pure of the
// DOM, so the tests drive them with fixtures and a fake `Api`.

import type { Api } from "./api/http";
import { newKey } from "./api/http";
import type { AgentView, ItemView, StateView, TaskView } from "./api/types";

export type ActionId =
  | "allow"
  | "allow_always"
  | "deny"
  | "reply"
  | "accept"
  | "send_back"
  | "message"
  | "resume"
  | "stop"
  | "dismiss"
  | "open_pr"
  | "open";

export interface Action {
  id: ActionId;
  label: string;
  /** Asks for text first: a note (`deny`, `send_back`) or the message itself. */
  text?: "note" | "required";
  /** The main action, drawn filled. */
  primary?: boolean;
}

export interface Need {
  item: ItemView;
  /** One line: what is asked. */
  title: string;
  /** The detail under it (the command, the question). */
  detail: string;
  /** Who asks, as the tree names it (`T-0004`, `executor`). */
  who: string;
  agent: AgentView | null;
  task: TaskView | null;
  actions: Action[];
}

const A = {
  allow: { id: "allow", label: "Allow", primary: true },
  always: { id: "allow_always", label: "Always" },
  deny: { id: "deny", label: "Deny…", text: "note" },
  reply: { id: "reply", label: "Reply…", text: "required", primary: true },
  accept: { id: "accept", label: "Accept", primary: true },
  back: { id: "send_back", label: "Send back…", text: "note" },
  message: { id: "message", label: "Message…", text: "required", primary: true },
  resume: { id: "resume", label: "Resume" },
  stop: { id: "stop", label: "Stop" },
  dismiss: { id: "dismiss", label: "Dismiss" },
  pr: { id: "open_pr", label: "Open PR", primary: true },
  open: { id: "open", label: "Open" },
} satisfies Record<string, Action>;

export function actionsFor(item: ItemView): Action[] {
  const k = item.kind;
  if (typeof k === "object") return [A.allow, A.always, A.deny, { ...A.open, label: "Terminal" }];
  switch (k) {
    case "question":
    case "child_question":
      return [A.reply, A.dismiss, A.open];
    case "plan_review":
      return [A.accept, A.back, A.open];
    case "stalled":
    case "exited_without_report":
    case "child_exited":
      return [A.message, A.resume, A.stop, A.dismiss, A.open];
    case "ci_failing":
      return [A.pr, { ...A.message, primary: false }, A.dismiss];
    default:
      return [A.dismiss, A.open];
  }
}

/** Pending items for a person, newest first. */
export function pending(v: StateView): ItemView[] {
  return v.items
    .filter((i) => i.to === "human" && i.status === "pending")
    .sort((a, b) => b.created - a.created || b.id - a.id);
}

function firstLine(s: string): string {
  const l = s.split("\n").find((x) => x.trim()) ?? "";
  return l.length > 200 ? `${l.slice(0, 199)}…` : l;
}

export function describe(v: StateView, item: ItemView): Need {
  const agent = v.agents.find((a) => a.id === item.from) ?? null;
  const task = agent?.task ? (v.tasks.find((t) => t.plan === planPart(agent.id) && t.id === agent.task) ?? null) : null;
  const who = item.from ? item.from.slice(item.from.indexOf("/") + 1) : "server";
  const k = item.kind;
  let title: string;
  let detail = firstLine(item.body);
  if (typeof k === "object") {
    title = `Allow ${k.permission.tool}?`;
    detail = k.permission.input_summary || detail;
  } else {
    switch (k) {
      case "question":
      case "child_question":
        title = "Question";
        break;
      case "plan_review": {
        const p = v.plans.find((x) => x.id === (item.from ? planPart(item.from) : ""));
        title = `Plan ready for review${p ? `: ${p.title}` : ""}`;
        if (p) detail = `${p.tasks.length} task${p.tasks.length === 1 ? "" : "s"}, ${p.repos.length} repo${p.repos.length === 1 ? "" : "s"}`;
        break;
      }
      case "stalled":
        title = "Stalled";
        break;
      case "exited_without_report":
      case "child_exited":
        title = "Exited without a report";
        break;
      case "ci_failing":
        title = `CI failing${task?.pr ? ` on #${task.pr.number}` : ""}`;
        break;
      default:
        title = k.replace(/_/g, " ");
    }
  }
  return { item, title, detail, who, agent, task, actions: actionsFor(item) };
}

export function needs(v: StateView): Need[] {
  return pending(v).map((i) => describe(v, i));
}

function planPart(agent: string): string {
  const at = agent.indexOf("/");
  return at < 0 ? agent : agent.slice(0, at);
}

/** What a view does for actions that leave the list (`open`, `open_pr`). */
export interface Nav {
  openAgent(agent: string): void;
  openPlan(plan: string): void;
  openUrl(url: string): void;
}

/**
 * Runs one action. Item answers carry an `Idempotency-Key`, so a retried
 * tap acts once. The agent actions (message, resume, stop) act on the agent
 * that raised the item, then dismiss the item: it is handled.
 */
export async function perform(api: Api, nav: Nav, need: Need, id: ActionId, text = ""): Promise<void> {
  const { item } = need;
  const agent = item.from;
  const answer = (action: Parameters<Api["answer"]>[1]["action"], t: string | null = null) =>
    api.answer(item.id, { action, text: t }, newKey());
  switch (id) {
    case "allow":
    case "allow_always":
    case "accept":
    case "dismiss":
      return answer(id);
    case "deny":
    case "send_back":
      return answer(id, text.trim() || null);
    case "reply":
      if (!text.trim()) throw new Error("A reply needs text.");
      return answer("reply", text.trim());
    case "message":
      if (!agent) throw new Error("No agent to message.");
      if (!text.trim()) throw new Error("A message needs text.");
      await api.message(agent, text.trim(), false);
      return answer("dismiss");
    case "resume":
      if (!agent) throw new Error("No agent to resume.");
      await api.resumeAgent(agent);
      return answer("dismiss");
    case "stop":
      if (!agent) throw new Error("No agent to stop.");
      await api.stopAgent(agent);
      return answer("dismiss");
    case "open_pr": {
      const url = need.task?.pr?.url ?? /https:\/\/\S+\/pull\/\d+/.exec(item.body)?.[0];
      if (!url) throw new Error("This item names no PR.");
      nav.openUrl(url);
      return;
    }
    case "open":
      if (item.kind === "plan_review" && agent) nav.openPlan(planPart(agent));
      else if (agent) nav.openAgent(agent);
      return;
  }
}
