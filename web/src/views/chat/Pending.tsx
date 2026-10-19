import { useState } from "react";
import { MessageCircleQuestionIcon, ShieldQuestionIcon } from "lucide-react";
import type { Api } from "@/api/http";
import type { StateView } from "@/api/types";
import { describe, perform, type ActionId, type Need, type Nav } from "@/needs";
import { Button } from "@/components/ui/button";
import { Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle } from "@/components/ui/item";
import { Textarea } from "@/components/ui/textarea";

const NO_NAV: Nav = { openAgent() {}, openPlan() {}, openUrl() {} };

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

/** One inline request in the Confirmation pattern: Allow / Always / Deny, or a reply. */
export function PendingRequest({ need, api }: { need: Need; api: Api }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [text, setText] = useState("");
  const question = typeof need.item.kind !== "object";

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

  return (
    <Item variant="outline" size="sm" role="group" aria-label={need.title} className="flex-wrap">
      <ItemMedia variant="icon">{question ? <MessageCircleQuestionIcon /> : <ShieldQuestionIcon />}</ItemMedia>
      <ItemContent className="min-w-0">
        <ItemTitle>{need.title}</ItemTitle>
        <ItemDescription className="font-mono break-all">{need.detail}</ItemDescription>
      </ItemContent>
      {question ? (
        <form
          className="flex w-full flex-col gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (text.trim()) void run("reply", text);
          }}
        >
          <Textarea
            aria-label="Reply"
            rows={2}
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder="Your answer…"
            className="field-sizing-content max-h-40"
          />
          <ItemActions className="justify-end">
            <Button type="submit" size="sm" disabled={busy || !text.trim()}>
              Reply
            </Button>
          </ItemActions>
        </form>
      ) : (
        <ItemActions>
          <Button size="sm" variant="outline" disabled={busy} onClick={() => void run("deny")}>
            Deny
          </Button>
          <Button size="sm" variant="outline" disabled={busy} onClick={() => void run("allow_always")}>
            Always
          </Button>
          <Button size="sm" disabled={busy} onClick={() => void run("allow")}>
            Allow
          </Button>
        </ItemActions>
      )}
      {error && (
        <p role="alert" className="w-full text-xs text-destructive">
          {error}
        </p>
      )}
    </Item>
  );
}
