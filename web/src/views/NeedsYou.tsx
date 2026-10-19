import {
  CheckCheckIcon,
  CircleXIcon,
  ClipboardCheckIcon,
  FileTextIcon,
  FlagIcon,
  MessageCircleQuestionIcon,
  PowerOffIcon,
  ShieldQuestionIcon,
  TriangleAlertIcon,
} from "lucide-react";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Item, ItemActions, ItemContent, ItemDescription, ItemFooter, ItemMedia, ItemTitle } from "@/components/ui/item";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { Textarea } from "@/components/ui/textarea";
import { relativeTime, useNow } from "@/lib/time";
import { ActionSheet, Swipe } from "./shell/phone";
import { cn } from "@/lib/utils";
import type { Api } from "../api/http";
import type { StateView } from "../api/types";
import { loadNotify } from "../push";
import {
  type Action,
  type ActionId,
  describe,
  groups,
  isLocal,
  type Nav,
  type Need,
  needs,
  notifyKind,
  pending,
  perform,
  prUrl,
} from "../needs";

interface Props {
  view: StateView;
  api: Api;
  nav: Nav;
  readOnly?: boolean;
  /** Touch layout: cards swipe, and a tap opens their details in a sheet. */
  phone?: boolean;
  /** Milliseconds since the epoch, for the age labels (tests pin it). */
  now?: number;
}

/** How long an answered card takes to collapse; matches `duration-150` below. */
export const EXIT_MS = 150;

// When this device last saw an item answered, for the empty state. Kept in
// localStorage so a reload still shows it; it is a convenience, not state.
const LAST_KEY = "cmux.lastAnswered";
const listeners = new Set<() => void>();

function readLast(): number | null {
  try {
    const n = Number(localStorage.getItem(LAST_KEY));
    return n > 0 ? n : null;
  } catch {
    return null;
  }
}

export function markAnswered(at = Date.now()): void {
  try {
    localStorage.setItem(LAST_KEY, String(at));
  } catch {
    // Not stored; the empty state shows no time.
  }
  for (const l of listeners) l();
}

function useLastAnswered(): number | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    readLast,
    () => null,
  );
}

const clock = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });

/** "45s", "12m", "3h 5m", "2d": how long an agent has been idle. */
export function idleFor(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h}h ${m % 60}m` : `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

/** Keys on a focused card: the action each one runs, when the card offers it. */
const KEYS: Record<string, ActionId[]> = {
  a: ["allow"],
  A: ["allow_always"],
  d: ["deny"],
  r: ["reply", "message"],
  o: ["open"],
};

function typing(t: EventTarget): boolean {
  return t instanceof HTMLElement && (t.tagName === "TEXTAREA" || t.tagName === "INPUT" || t.isContentEditable);
}

/** The needs-you inbox (UI design 2.3): grouped by plan, triaged by keyboard. */
export function NeedsYou({ view, api, nav, readOnly, phone, now }: Props) {
  const live = useNow(30_000);
  const t = now ?? live;
  const last = useLastAnswered();
  // Answered cards: `leaving` while they collapse, `gone` after. A failed
  // answer takes the card out of both, so it comes back.
  const [leaving, setLeaving] = useState<ReadonlySet<number>>(new Set());
  const [gone, setGone] = useState<ReadonlySet<number>>(new Set());
  const [active, setActive] = useState<number | null>(null);
  const [inside, setInside] = useState(false);
  const cards = useRef(new Map<number, HTMLDivElement>());
  const listRef = useRef<HTMLDivElement>(null);

  const visible = needs(view).filter((n) => !gone.has(n.item.id));
  const grouped = groups(view, visible);
  const flat = grouped.flatMap((g) => g.needs).filter((n) => !leaving.has(n.item.id));
  const current = flat.find((n) => n.item.id === active) ?? flat[0] ?? null;

  const focus = (id: number | undefined) => {
    if (id === undefined) return;
    setActive(id);
    cards.current.get(id)?.focus();
  };

  const edit = (ids: ReadonlySet<number>, id: number, on: boolean) => {
    const next = new Set(ids);
    if (on) next.add(id);
    else next.delete(id);
    return next;
  };

  const act = async (need: Need, a: Action, text = "") => {
    if (isLocal(a.id)) {
      try {
        await perform(api, nav, need, a.id, text);
      } catch (e) {
        toast.error(`${a.label} failed`, { description: e instanceof Error ? e.message : String(e) });
      }
      return;
    }
    if (readOnly) return;
    const id = need.item.id;
    // Move the focus on before the card goes, so triage keeps its place.
    const at = flat.findIndex((n) => n.item.id === id);
    const next = flat[at + 1] ?? flat[at - 1];
    if (listRef.current?.contains(document.activeElement)) focus(next?.item.id);
    else if (next) setActive(next.item.id);
    setLeaving((s) => edit(s, id, true));
    const timer = setTimeout(() => setGone((s) => edit(s, id, true)), EXIT_MS);
    try {
      await perform(api, nav, need, a.id, text);
      markAnswered();
    } catch (e) {
      clearTimeout(timer);
      setLeaving((s) => edit(s, id, false));
      setGone((s) => edit(s, id, false));
      toast.error(`${a.label.replace("…", "")} failed`, { description: e instanceof Error ? e.message : String(e) });
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (typing(e.target) || e.metaKey || e.ctrlKey || e.altKey) return;
    const at = current ? flat.indexOf(current) : -1;
    if (e.key === "j" || e.key === "ArrowDown") {
      e.preventDefault();
      focus(flat[Math.min(flat.length - 1, at + 1)]?.item.id);
    } else if (e.key === "k" || e.key === "ArrowUp") {
      e.preventDefault();
      focus(flat[Math.max(0, at - 1)]?.item.id);
    }
  };

  if (visible.length === 0)
    return (
      <Empty>
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <CheckCheckIcon />
          </EmptyMedia>
          <EmptyTitle>Nothing needs you</EmptyTitle>
          <EmptyDescription>
            {last ? (
              <>
                Last answered at <time dateTime={new Date(last).toISOString()}>{clock.format(last)}</time>.
              </>
            ) : (
              "Requests from agents show up here."
            )}
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    );

  return (
    <div
      ref={listRef}
      className="flex flex-col gap-4 p-3"
      onKeyDown={onKeyDown}
      onFocus={(e) => {
        setInside(true);
        const id = Number((e.target as HTMLElement).closest<HTMLElement>("[data-item]")?.dataset.item);
        if (id) setActive(id);
      }}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setInside(false);
      }}
    >
      {grouped.map((g) => (
        <section key={g.plan ?? ""} aria-label={g.title} className="flex flex-col gap-2">
          <h3 className="flex items-center gap-2 px-1 text-xs font-medium text-muted-foreground">
            <span className="truncate">{g.title}</span>
            <span className="counter">{g.needs.filter((n) => !leaving.has(n.item.id)).length}</span>
          </h3>
          <div role="list" aria-label={g.title} className="flex flex-col gap-2">
            {g.needs.map((n) => (
              <NeedCard
                key={n.item.id}
                need={n}
                view={view}
                now={t}
                readOnly={readOnly}
                phone={phone}
                leaving={leaving.has(n.item.id)}
                tabbable={current?.item.id === n.item.id}
                hints={inside && current?.item.id === n.item.id}
                onAct={act}
                setRef={(el) => {
                  if (el) cards.current.set(n.item.id, el);
                  else cards.current.delete(n.item.id);
                }}
              />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}

function KindIcon({ need }: { need: Need }) {
  const k = need.item.kind;
  if (typeof k === "object") return <ShieldQuestionIcon aria-hidden className="text-status-needs" />;
  switch (k) {
    case "question":
    case "child_question":
      return <MessageCircleQuestionIcon aria-hidden className="text-status-needs" />;
    case "plan_review":
      return <ClipboardCheckIcon aria-hidden className="text-status-needs" />;
    case "stalled":
      return <TriangleAlertIcon aria-hidden className="text-status-stalled" />;
    case "exited_without_report":
    case "child_exited":
      return <PowerOffIcon aria-hidden className="text-status-failing" />;
    case "ci_failing":
      return <CircleXIcon aria-hidden className="text-status-failing" />;
    case "child_report":
      return <FileTextIcon aria-hidden className="text-muted-foreground" />;
    default:
      return <FlagIcon aria-hidden className="text-status-needs" />;
  }
}

interface CardProps {
  need: Need;
  view: StateView;
  now: number;
  readOnly?: boolean;
  phone?: boolean;
  leaving: boolean;
  /** The card that Tab lands on (roving focus). */
  tabbable: boolean;
  /** Show the key hints: the card has the focus. */
  hints: boolean;
  onAct: (need: Need, a: Action, text?: string) => void;
  setRef: (el: HTMLDivElement | null) => void;
}

/** One item in the Confirmation pattern: what is asked, then its actions. */
function NeedCard({ need, view, now, readOnly, phone, leaving, tabbable, hints, onAct, setRef }: CardProps) {
  const { item } = need;
  const k = item.kind;
  const question = k === "question" || k === "child_question";
  const [asking, setAsking] = useState<Action | null>(null);
  const [text, setText] = useState("");
  const [sheet, setSheet] = useState<"details" | "deny" | null>(null);
  const reply = useRef<HTMLTextAreaElement>(null);
  const self = useRef<HTMLDivElement | null>(null);
  const has = (id: ActionId) => need.actions.find((a) => a.id === id);
  const disabled = (a: Action) => readOnly === true && !isLocal(a.id);

  const tap = (a: Action) => {
    if (disabled(a)) return;
    if (a.id === "reply" && question) return reply.current?.focus();
    if (a.text) return setAsking(a);
    onAct(need, a);
  };

  const send = (a: Action | null) => {
    if (!a || disabled(a) || (a.text === "required" && !text.trim())) return;
    onAct(need, a, text);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape" && typing(e.target)) {
      setAsking(null);
      self.current?.focus();
      return;
    }
    if (typing(e.target) || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === "Enter") {
      // A focused button handles its own Enter.
      if (e.target !== e.currentTarget) return;
      const primary = need.actions.find((a) => a.primary) ?? need.actions[0];
      if (primary) {
        e.preventDefault();
        tap(primary);
      }
      return;
    }
    // `d` denies at once (the note is optional); the button asks for one.
    const a = KEYS[e.key]?.map(has).find(Boolean);
    if (!a) return;
    e.preventDefault();
    e.stopPropagation();
    if (a.id === "deny") send(a);
    else tap(a);
  };

  const agent = need.agent;
  const idle = agent?.idle_since ?? item.created;
  const pr = k === "ci_failing" ? prUrl(need) : null;
  const plan = k === "plan_review" ? view.plans.find((p) => item.from?.startsWith(`${p.id}/`)) : undefined;
  const buttons = need.actions.filter((a) => !(question && a.id === "reply"));
  const swipe = phone ? swipeActions(need) : null;
  const deny = has("deny");

  return (
    <div
      ref={(el) => {
        self.current = el;
        setRef(el);
      }}
      role="listitem"
      aria-label={need.title}
      aria-hidden={leaving || undefined}
      data-item={item.id}
      data-leaving={leaving || undefined}
      tabIndex={tabbable ? 0 : -1}
      onKeyDown={onKeyDown}
      className={cn(
        "grid rounded-lg outline-none transition-[grid-template-rows,opacity,scale] duration-150 ease-out focus-visible:ring-2 focus-visible:ring-ring",
        leaving ? "pointer-events-none grid-rows-[0fr] scale-98 opacity-0" : "grid-rows-[1fr]",
      )}
    >
      <div className="min-h-0 overflow-hidden">
        <Swipe
          disabled={!phone || leaving}
          right={
            swipe?.right && !disabled(swipe.right)
              ? { label: swipe.right.label.replace("…", ""), run: () => tap(swipe.right!) }
              : null
          }
          left={
            swipe?.left && !disabled(swipe.left)
              ? {
                  label: swipe.left.label.replace("…", ""),
                  destructive: true,
                  // A denial cannot be taken back: confirm it first.
                  run: () => (swipe.left!.id === "deny" ? setSheet("deny") : onAct(need, swipe.left!)),
                }
              : null
          }
        >
          <Item variant="outline" size="sm" className="items-start">
            <ItemMedia variant="icon">
              <KindIcon need={need} />
            </ItemMedia>
            <ItemContent
              className="min-w-0"
              onClick={phone ? () => setSheet("details") : undefined}
              role={phone ? "button" : undefined}
              aria-label={phone ? `Details: ${need.title}` : undefined}
            >
              <ItemTitle className="w-full">
                <span className="min-w-0 truncate">{need.title}</span>
                <span className="ml-auto shrink-0 font-normal text-muted-foreground">
                  <span className="font-mono">{need.who}</span> ·{" "}
                  <time className="counter">{relativeTime(item.created, now)}</time>
                </span>
              </ItemTitle>
              {typeof k === "object" ? (
                <pre className="mt-1 max-h-40 overflow-auto rounded-md bg-muted px-2 py-1.5 font-mono text-xs break-all whitespace-pre-wrap">
                  {k.permission.input_summary || need.detail}
                </pre>
              ) : k === "plan_review" ? (
                <ItemDescription>
                  {plan ? `${plan.tasks.length} task${plan.tasks.length === 1 ? "" : "s"}` : need.detail}
                </ItemDescription>
              ) : k === "stalled" ? (
                <ItemDescription>Idle for {idleFor(now - idle)}</ItemDescription>
              ) : k === "ci_failing" ? (
                <ItemDescription className="line-clamp-2">
                  {pr ? (
                    <a href={pr} target="_blank" rel="noopener noreferrer" className="font-mono underline underline-offset-2">
                      {need.task?.pr ? `#${need.task.pr.number}` : pr}
                    </a>
                  ) : (
                    need.detail
                  )}
                  {need.task?.pr?.ci_failures ? ` · ${need.task.pr.ci_failures} failing` : ""}
                </ItemDescription>
              ) : question || k === "child_report" ? (
                <ItemDescription className="line-clamp-6 whitespace-pre-wrap">{item.body}</ItemDescription>
              ) : (
                need.detail && <ItemDescription className="line-clamp-3">{need.detail}</ItemDescription>
              )}
            </ItemContent>
            <ItemFooter className="flex-col items-stretch gap-2">
              {question && (
                <form
                  className="flex flex-col gap-2"
                  onSubmit={(e) => {
                    e.preventDefault();
                    send(has("reply") ?? null);
                  }}
                >
                  <Textarea
                    ref={reply}
                    aria-label="Reply"
                    placeholder="Your answer…"
                    rows={2}
                    value={text}
                    disabled={readOnly}
                    onChange={(e) => setText(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                        e.preventDefault();
                        send(has("reply") ?? null);
                      }
                    }}
                    className="field-sizing-content max-h-40"
                  />
                </form>
              )}
              {asking && (
                <form
                  className="flex flex-col gap-2"
                  onSubmit={(e) => {
                    e.preventDefault();
                    send(asking);
                  }}
                >
                  <Textarea
                    aria-label={asking.text === "note" ? "Note" : asking.label.replace("…", "")}
                    placeholder={asking.text === "note" ? "Optional note" : "Text"}
                    value={text}
                    rows={2}
                    onChange={(e) => setText(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                        e.preventDefault();
                        send(asking);
                      }
                    }}
                    autoFocus
                  />
                  <ItemActions>
                    <Button type="submit" size="sm" disabled={asking.text === "required" && !text.trim()}>
                      {asking.label.replace("…", "")}
                    </Button>
                    <Button type="button" size="sm" variant="ghost" onClick={() => setAsking(null)}>
                      Cancel
                    </Button>
                  </ItemActions>
                </form>
              )}
              {!asking && (
                <ItemActions className="flex-wrap">
                  {question && (
                    <Button
                      type="button"
                      size="sm"
                      disabled={disabled(has("reply")!) || !text.trim()}
                      onClick={() => send(has("reply") ?? null)}
                    >
                      Reply
                    </Button>
                  )}
                  {buttons.map((a) => (
                    <Button
                      key={a.id}
                      type="button"
                      size="sm"
                      variant={a.primary ? "default" : a.id === "open" ? "ghost" : "outline"}
                      disabled={disabled(a)}
                      onClick={() => tap(a)}
                    >
                      {a.label}
                    </Button>
                  ))}
                </ItemActions>
              )}
              {hints && <Hints need={need} question={question} />}
            </ItemFooter>
          </Item>
        </Swipe>
      </div>
      {phone && (
        <ActionSheet
          open={sheet !== null}
          onOpenChange={(o) => !o && setSheet(null)}
          title={sheet === "deny" ? `Deny: ${need.title}` : need.title}
          description={
            sheet === "deny" ? (
              "The agent gets a refusal. This cannot be undone."
            ) : (
              <>
                <span className="font-mono">{need.who}</span> · {relativeTime(item.created, now)}
              </>
            )
          }
          actions={
            sheet === "deny"
              ? deny
                ? [{ id: "deny", label: "Deny", destructive: true, run: () => onAct(need, deny) }]
                : []
              : buttons.map((a) => ({
                  id: a.id,
                  label: a.label,
                  destructive: a.id === "deny",
                  disabled: disabled(a),
                  run: () => (a.id === "deny" ? setSheet("deny") : tap(a)),
                }))
          }
        >
          {sheet === "details" && <Details need={need} />}
        </ActionSheet>
      )}
    </div>
  );
}

/** Swipe right runs Allow (or Open), left denies (or dismisses). */
export function swipeActions(need: Need): { right: Action | null; left: Action | null } {
  const has = (id: ActionId) => need.actions.find((a) => a.id === id) ?? null;
  const primary = need.actions.find((a) => a.primary && !a.text) ?? null;
  return { right: has("allow") ?? primary ?? has("open"), left: has("deny") ?? has("dismiss") };
}

/** Everything the card clips: the full input, body, or detail. */
function Details({ need }: { need: Need }) {
  const k = need.item.kind;
  const text = typeof k === "object" ? k.permission.input_summary || need.detail : need.item.body || need.detail;
  if (!text) return null;
  return (
    <pre className="max-h-[40dvh] overflow-auto rounded-md bg-muted px-3 py-2 font-mono text-xs break-all whitespace-pre-wrap select-text">
      {text}
    </pre>
  );
}

function Hints({ need, question }: { need: Need; question: boolean }) {
  const ids = new Set(need.actions.map((a) => a.id));
  const primary = need.actions.find((a) => a.primary);
  const hint = (keys: string[], label: string) => (
    <span key={label} className="inline-flex items-center gap-1">
      <KbdGroup>
        {keys.map((key) => (
          <Kbd key={key}>{key}</Kbd>
        ))}
      </KbdGroup>
      {label}
    </span>
  );
  return (
    <div aria-hidden className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
      {primary && hint(["↵"], primary.label.replace("…", ""))}
      {ids.has("allow") && hint(["A"], "Allow")}
      {ids.has("allow_always") && hint(["⇧", "A"], "Always")}
      {ids.has("deny") && hint(["D"], "Deny")}
      {(ids.has("reply") || ids.has("message")) && hint(["R"], question ? "Reply" : "Message")}
      {question && hint(["⌘", "↵"], "Send")}
      {ids.has("open") && hint(["O"], "Open")}
      {hint(["J", "K"], "Move")}
    </div>
  );
}

// The attention signals outside the inbox: the tab title, the favicon dot,
// and a toast for a new item while the user looks at another view.

let iconHref: string | null = null;
let iconType: string | null = null;
let iconRun = 0;

/** Draws the favicon with a dot on a canvas, as a data: URL (the CSP allows data: images). */
async function badgeFavicon(on: boolean): Promise<void> {
  const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
  if (!link) return;
  const run = ++iconRun;
  iconHref ??= link.href;
  iconType ??= link.type;
  if (!on) {
    link.href = iconHref;
    link.type = iconType;
    return;
  }
  try {
    const img = new Image();
    img.src = iconHref;
    await img.decode();
    const c = document.createElement("canvas");
    c.width = c.height = 64;
    const g = c.getContext("2d");
    if (!g || run !== iconRun) return;
    g.drawImage(img, 0, 0, 64, 64);
    g.beginPath();
    g.arc(48, 16, 13, 0, 2 * Math.PI);
    g.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--status-needs").trim() || "#e5484d";
    g.fill();
    g.lineWidth = 4;
    g.strokeStyle = "#fff";
    g.stroke();
    link.type = "image/png";
    link.href = c.toDataURL("image/png");
  } catch {
    // No canvas or the icon did not load: the plain favicon stays.
  }
}

/**
 * Keeps the user aware of pending items from any view: "(N) cmux" in the
 * title, a dot on the favicon, and a toast with an Open action for each new
 * item while `inInbox` is false (only kinds the notification settings allow).
 */
export function useAttention(view: StateView | null, inInbox: boolean, openInbox: () => void): void {
  const list = view ? pending(view) : [];
  const count = list.length;
  const seen = useRef<Set<number> | null>(null);

  useEffect(() => {
    document.title = count > 0 ? `(${count}) cmux` : "cmux";
  }, [count]);

  useEffect(() => {
    void badgeFavicon(count > 0);
  }, [count > 0]);

  const ids = list.map((i) => i.id).join(",");
  useEffect(() => {
    if (!view) return;
    const now = new Set(list.map((i) => i.id));
    const before = seen.current;
    seen.current = now;
    if (!before) return;
    if ([...before].some((id) => !now.has(id))) markAnswered();
    if (inInbox) return;
    const on = loadNotify();
    for (const i of list) {
      if (before.has(i.id)) continue;
      const kind = notifyKind(i);
      if (!kind || on[kind] === false) continue;
      const n = describe(view, i);
      toast(n.title, {
        id: `need-${i.id}`,
        description: `${n.who}${n.detail ? ` · ${n.detail}` : ""}`,
        action: { label: "Open", onClick: openInbox },
      });
    }
  }, [ids]);
}
