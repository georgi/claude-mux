import { useEffect, useRef, useState } from "react";
import { ArrowUpIcon, ChevronsUpIcon, MailIcon } from "lucide-react";
import type { Api } from "@/api/http";
import type { StateView } from "@/api/types";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupTextarea } from "@/components/ui/input-group";
import { Kbd } from "@/components/ui/kbd";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

/** A message that waits in the queue for the agent's next turn boundary. */
export interface QueuedMessage {
  id: number;
  text: string;
  /** It interrupts the agent rather than waiting for the turn to end. */
  steer: boolean;
}

/** The messages queued for `agent` and not delivered yet, oldest first. */
export function queuedMessages(v: StateView, agent: string): QueuedMessage[] {
  return v.items
    .filter(
      (i) =>
        typeof i.to === "object" &&
        i.to.agent === agent &&
        i.status === "pending" &&
        (i.kind === "user_message" || i.kind === "steer"),
    )
    .sort((a, b) => a.created - b.created || a.id - b.id)
    .map((i) => ({ id: i.id, text: i.body, steer: i.kind === "steer" }));
}

const draftKey = (agent: string) => `cmux.draft.${agent}`;

/** The unsent text for `agent`. Storage can be missing or blocked; the draft is then empty. */
export function loadDraft(agent: string): string {
  try {
    return localStorage.getItem(draftKey(agent)) ?? "";
  } catch {
    return "";
  }
}

function saveDraft(agent: string, text: string) {
  try {
    if (text) localStorage.setItem(draftKey(agent), text);
    else localStorage.removeItem(draftKey(agent));
  } catch {
    // A private window or full storage: the draft lives only while the page does.
  }
}

const QUOTE = "cmux:quote";

/** Appends `text` to the composer of `agent` and focuses it ("Quote in reply"). */
export function quoteInto(agent: string, text: string) {
  window.dispatchEvent(new CustomEvent(QUOTE, { detail: { agent, text } }));
}

const mac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
const MOD = mac ? "⌘" : "Ctrl";

interface ComposerProps {
  agent: string;
  api: Api;
  /** Messages queued for this agent and not delivered yet. */
  queued: QueuedMessage[];
  readOnly?: boolean;
}

/**
 * The message bar: **Enter** queues the message for the agent's next turn
 * boundary; with **Steer** on (or with ⌘/Ctrl+Enter) it interrupts the
 * agent. Neither types into the terminal. The draft stays per agent.
 */
export function Composer({ agent, api, queued, readOnly }: ComposerProps) {
  const [text, setText] = useState(() => loadDraft(agent));
  const [steer, setSteer] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const area = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    setText(loadDraft(agent));
    setError(null);
    setSteer(false);
  }, [agent]);

  const edit = (t: string) => {
    setText(t);
    saveDraft(agent, t);
  };

  useEffect(() => {
    const on = (e: Event) => {
      const d = (e as CustomEvent<{ agent: string; text: string }>).detail;
      if (d.agent !== agent || readOnly) return;
      setText((t) => {
        const next = t && !t.endsWith("\n") ? `${t}\n\n${d.text}` : t + d.text;
        saveDraft(agent, next);
        return next;
      });
      area.current?.focus();
    };
    window.addEventListener(QUOTE, on);
    return () => window.removeEventListener(QUOTE, on);
  }, [agent, readOnly]);

  const send = async (interrupt: boolean) => {
    const t = text.trim();
    if (!t || busy || readOnly) return;
    setBusy(true);
    setError(null);
    try {
      await api.message(agent, t, interrupt);
      edit("");
      setSteer(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
      area.current?.focus();
    }
  };

  const name = agent.slice(agent.indexOf("/") + 1);
  const empty = !text.trim();
  const why = readOnly
    ? "This device is read-only. Pair it with full access to send messages."
    : busy
      ? "Sending…"
      : empty
        ? "Type a message first"
        : null;

  return (
    <form
      className="shrink-0 border-t bg-background p-2 pr-[max(0.5rem,env(safe-area-inset-right))] pl-[max(0.5rem,env(safe-area-inset-left))]"
      onSubmit={(e) => {
        e.preventDefault();
        void send(steer);
      }}
    >
      {queued.length > 0 && (
        <section aria-label="Queued messages" className="mb-2 rounded-lg border bg-muted/40 px-3 py-2 text-xs">
          <h3 className="flex items-center gap-1.5 font-medium text-muted-foreground">
            <MailIcon className="size-3.5" aria-hidden="true" /> {queued.length} queued
          </h3>
          <ol className="mt-1 flex max-h-24 flex-col gap-1 overflow-y-auto">
            {queued.map((q) => (
              <li key={q.id} className="flex min-w-0 items-center gap-2">
                {q.steer && <ChevronsUpIcon className="size-3.5 shrink-0 text-muted-foreground" aria-label="Steer" />}
                <span className="truncate">{q.text}</span>
              </li>
            ))}
          </ol>
        </section>
      )}
      <Tooltip disabled={!readOnly}>
        <TooltipTrigger render={<div />}>
          <InputGroup>
            <InputGroupTextarea
              ref={area}
              aria-label="Message"
              rows={1}
              className="field-sizing-content max-h-40 min-h-10"
              placeholder={readOnly ? "Read-only device" : `Message ${name}…`}
              disabled={readOnly}
              value={text}
              onChange={(e) => edit(e.target.value)}
              onKeyDown={(e) => {
                if (e.nativeEvent.isComposing) return;
                if (e.key === "Escape") {
                  e.currentTarget.blur();
                } else if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  void send(steer || e.metaKey || e.ctrlKey);
                }
              }}
            />
            <InputGroupAddon align="block-end">
              <ToggleGroup
                size="sm"
                variant="outline"
                className="ml-auto"
                value={steer ? ["steer"] : []}
                onValueChange={(v) => setSteer(v.includes("steer"))}
              >
                <Tooltip>
                  <TooltipTrigger
                    render={
                      <ToggleGroupItem value="steer" aria-label="Steer: interrupt the agent" disabled={readOnly} className="gap-1.5 max-sm:h-11 max-sm:min-w-11" />
                    }
                  >
                    <ChevronsUpIcon /> Steer
                    <Kbd className="hidden sm:inline-flex">{MOD}↵</Kbd>
                  </TooltipTrigger>
                  <TooltipContent>
                    Interrupt the agent now, not at its next turn <Kbd>{MOD}↵</Kbd>
                  </TooltipContent>
                </Tooltip>
              </ToggleGroup>
              <Tooltip>
                <TooltipTrigger render={<span className="inline-flex" />}>
                  <InputGroupButton
                    type="submit"
                    variant="default"
                    size="icon-sm"
                    className="max-sm:size-11"
                    aria-label={steer ? "Steer now" : "Send at the next turn"}
                    disabled={!!why}
                  >
                    {steer ? <ChevronsUpIcon /> : <ArrowUpIcon />}
                  </InputGroupButton>
                </TooltipTrigger>
                <TooltipContent>
                  {why ?? (
                    <>
                      {steer ? "Steer now" : "Send at the next turn"} <Kbd>↵</Kbd>
                    </>
                  )}
                </TooltipContent>
              </Tooltip>
            </InputGroupAddon>
          </InputGroup>
        </TooltipTrigger>
        <TooltipContent>This device is read-only. Pair it with full access to send messages.</TooltipContent>
      </Tooltip>
      {error && (
        <p role="alert" className="mt-1 text-xs text-destructive">
          {error}
        </p>
      )}
    </form>
  );
}
