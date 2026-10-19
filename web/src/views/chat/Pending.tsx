import { useState } from "react";
import { MessageCircleQuestionIcon, ShieldQuestionIcon } from "lucide-react";
import type { Api } from "@/api/http";
import type { StateView } from "@/api/types";
import { describe, perform, type ActionId, type Need, type Nav } from "@/needs";
import { Button } from "@/components/ui/button";
import { CodeBlock, CodeBlockContent, CodeBlockLine } from "@/components/ui/code-block";
import { Kbd } from "@/components/ui/kbd";
import { Textarea } from "@/components/ui/textarea";

const NO_NAV: Nav = { openAgent() {}, openPlan() {}, openUrl() {} };

const MOD = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent) ? "⌘" : "Ctrl";

/** Pending permission requests and questions that `agent` raised for a person. */
export function pendingFor(v: StateView, agent: string): Need[] {
  return v.items
    .filter(
      (i) =>
        i.to === "human" &&
        i.status === "pending" &&
        i.from === agent &&
        (typeof i.kind === "object" || i.kind === "question" || i.kind === "child_question"),
    )
    .sort((a, b) => a.created - b.created || a.id - b.id)
    .map((i) => describe(v, i));
}

/**
 * One inline request in the Confirmation pattern: the tool and its input as
 * code with Allow / Always / Deny, or a question with a reply box.
 */
export function PendingRequest({ need, api, readOnly }: { need: Need; api: Api; readOnly?: boolean }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [text, setText] = useState("");
  const kind = need.item.kind;
  const tool = typeof kind === "object" ? kind.permission.tool : null;
  const off = busy || readOnly;

  const run = async (id: ActionId, t = "") => {
    setBusy(true);
    setError(null);
    try {
      await perform(api, NO_NAV, need, id, t);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };
  const reply = () => {
    if (text.trim() && !off) void run("reply", text);
  };

  return (
    <section
      role="group"
      aria-label={need.title}
      className="flex min-w-0 flex-col gap-3 rounded-xl border border-status-needs/40 bg-card p-3 text-sm shadow-xs"
    >
      <header className="flex items-center gap-2">
        {tool ? (
          <ShieldQuestionIcon className="size-4 text-status-needs" aria-hidden="true" />
        ) : (
          <MessageCircleQuestionIcon className="size-4 text-status-needs" aria-hidden="true" />
        )}
        <h3 className="font-medium">{need.title}</h3>
        {tool && <span className="ml-auto font-mono text-xs text-muted-foreground">{tool}</span>}
      </header>
      {tool ? (
        need.detail && (
          <CodeBlock size="sm">
            <CodeBlockContent wrap aria-label={`${tool} input`}>
              {need.detail.split("\n").map((l, i) => (
                <CodeBlockLine key={i}>{l}</CodeBlockLine>
              ))}
            </CodeBlockContent>
          </CodeBlock>
        )
      ) : (
        <p className="whitespace-pre-wrap">{need.item.body}</p>
      )}
      {tool ? (
        <div className="flex flex-wrap justify-end gap-2">
          <Button size="sm" variant="outline" disabled={off} onClick={() => void run("deny")}>
            Deny
          </Button>
          <Button size="sm" variant="outline" disabled={off} onClick={() => void run("allow_always")}>
            Always
          </Button>
          <Button size="sm" disabled={off} onClick={() => void run("allow")}>
            Allow
          </Button>
        </div>
      ) : (
        <form
          className="flex flex-col gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            reply();
          }}
        >
          <Textarea
            aria-label="Reply"
            rows={2}
            value={text}
            disabled={readOnly}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.nativeEvent.isComposing) {
                e.preventDefault();
                reply();
              }
            }}
            placeholder="Your answer…"
            className="field-sizing-content max-h-40"
          />
          <div className="flex justify-end">
            <Button type="submit" size="sm" disabled={off || !text.trim()}>
              Reply <Kbd className="bg-primary-foreground/15 text-primary-foreground">{MOD}↵</Kbd>
            </Button>
          </div>
        </form>
      )}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
    </section>
  );
}
