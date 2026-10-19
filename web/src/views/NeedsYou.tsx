import { CheckCheckIcon, FlagIcon, Loader2Icon, TriangleAlertIcon } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import {
  Item,
  ItemActions,
  ItemContent,
  ItemDescription,
  ItemFooter,
  ItemGroup,
  ItemMedia,
  ItemTitle,
} from "@/components/ui/item";
import { Textarea } from "@/components/ui/textarea";
import type { Api } from "../api/http";
import type { StateView } from "../api/types";
import { type Action, type Nav, type Need, needs, perform } from "../needs";

interface Props {
  view: StateView;
  api: Api;
  nav: Nav;
  readOnly?: boolean;
  /** Milliseconds since the epoch, for the age labels (tests pin it). */
  now?: number;
}

function age(created: number, now: number): string {
  const s = Math.max(0, Math.floor((now - created) / 1000));
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

/** The needs-you list (UI design 2.3), each item with its one-tap actions. */
export function NeedsYou({ view, api, nav, readOnly, now }: Props) {
  const list = needs(view);
  const t = now ?? Date.now();
  if (list.length === 0)
    return (
      <Empty>
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <CheckCheckIcon />
          </EmptyMedia>
          <EmptyTitle>All clear</EmptyTitle>
          <EmptyDescription>Nothing needs you.</EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  return (
    <ItemGroup aria-label="Needs you" className="gap-2 p-3">
      {list.map((n) => (
        <NeedCard key={n.item.id} need={n} api={api} nav={nav} readOnly={readOnly} age={age(n.item.created, t)} />
      ))}
    </ItemGroup>
  );
}

function NeedCard({ need, api, nav, readOnly, age }: { need: Need; api: Api; nav: Nav; readOnly?: boolean; age: string }) {
  const [asking, setAsking] = useState<Action | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (a: Action, t = "") => {
    setBusy(true);
    setError(null);
    try {
      await perform(api, nav, need, a.id, t);
      setAsking(null);
      setText("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const tap = (a: Action) => {
    if (a.text) {
      setAsking(a);
      setError(null);
      return;
    }
    void run(a);
  };

  const submit = (ev: React.FormEvent) => {
    ev.preventDefault();
    if (asking) void run(asking, text);
  };

  const local = (a: Action) => a.id === "open" || a.id === "open_pr";
  const stalled = need.item.kind === "stalled";
  return (
    <Item variant="outline" size="sm" role="listitem" data-item={need.item.id}>
      <ItemMedia variant="icon">
        {stalled ? (
          <TriangleAlertIcon aria-label="Stalled" className="text-destructive" />
        ) : (
          <FlagIcon aria-label="Needs you" className="text-primary" />
        )}
      </ItemMedia>
      <ItemContent>
        <ItemTitle>
          {need.who}
          <span className="font-normal text-muted-foreground">· {age}</span>
        </ItemTitle>
        <ItemDescription className="line-clamp-3">{need.title}</ItemDescription>
        {need.detail && (
          <code className="mt-1 block max-w-full truncate rounded-md bg-muted px-2 py-1 text-xs">{need.detail}</code>
        )}
      </ItemContent>
      <ItemFooter className="flex-col items-stretch gap-2">
        {asking ? (
          <form onSubmit={submit} className="flex flex-col gap-2">
            <Textarea
              aria-label={asking.text === "note" ? "Note" : asking.label.replace("…", "")}
              placeholder={asking.text === "note" ? "Optional note" : "Text"}
              value={text}
              rows={2}
              onChange={(e) => setText(e.target.value)}
              autoFocus
            />
            <ItemActions>
              <Button type="submit" size="sm" disabled={busy || (asking.text === "required" && !text.trim())}>
                {busy && <Loader2Icon className="animate-spin" />}
                {asking.label.replace("…", "")}
              </Button>
              <Button type="button" size="sm" variant="ghost" onClick={() => setAsking(null)}>
                Cancel
              </Button>
            </ItemActions>
          </form>
        ) : (
          <ItemActions className="flex-wrap">
            {need.actions.map((a) => (
              <Button
                key={a.id}
                type="button"
                size="sm"
                variant={a.primary ? "default" : "outline"}
                disabled={busy || (readOnly && !local(a))}
                onClick={() => tap(a)}
              >
                {a.label}
              </Button>
            ))}
          </ItemActions>
        )}
        {error && (
          <p role="alert" className="text-xs text-destructive">
            {error}
          </p>
        )}
      </ItemFooter>
    </Item>
  );
}
