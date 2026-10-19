import { CheckIcon, CircleAlertIcon } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { errText } from "../board/state";
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";

/** Loads `f` when `deps` change; the third value reloads. */
export function useLoad<T>(f: () => Promise<T>, deps: unknown[]): [T | null, string | null, () => void] {
  const [v, setV] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [n, setN] = useState(0);
  useEffect(() => {
    let live = true;
    f()
      .then((x) => {
        if (!live) return;
        setV(x);
        setError(null);
      })
      .catch((e: unknown) => live && setError(errText(e)));
    return () => {
      live = false;
    };
    // biome-ignore lint/correctness/useExhaustiveDependencies: f changes each render; deps drive reloads.
  }, [...deps, n]);
  return [v, error, useCallback(() => setN((x) => x + 1), [])];
}

export type Save = { state: "idle" } | { state: "busy" } | { state: "saved" } | { state: "error"; text: string };

/**
 * The inline state of a save: `run` marks it busy, then saved (for a few
 * seconds) or the error (until the next save).
 */
export function useSave(): [Save, (f: () => Promise<unknown>) => Promise<boolean>] {
  const [s, setS] = useState<Save>({ state: "idle" });
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const run = useCallback(async (f: () => Promise<unknown>) => {
    clearTimeout(timer.current);
    setS({ state: "busy" });
    try {
      await f();
      setS({ state: "saved" });
      timer.current = setTimeout(() => setS({ state: "idle" }), 2500);
      return true;
    } catch (e) {
      setS({ state: "error", text: errText(e) });
      return false;
    }
  }, []);
  return [s, run];
}

/** The inline saved/error line next to a control. */
export function SaveState({ save, className }: { save: Save; className?: string }) {
  return (
    <span role="status" className={cn("inline-flex min-h-5 items-center gap-1 text-xs", className)}>
      {save.state === "busy" && <Spinner className="size-3 text-muted-foreground" />}
      {save.state === "saved" && (
        <span className="inline-flex items-center gap-1 text-muted-foreground">
          <CheckIcon aria-hidden className="size-3" /> Saved
        </span>
      )}
      {save.state === "error" && (
        <span className="inline-flex items-center gap-1 text-destructive">
          <CircleAlertIcon aria-hidden className="size-3" /> {save.text}
        </span>
      )}
    </span>
  );
}

export function Err({ text }: { text: string | null }) {
  return text ? (
    <p role="alert" className="text-sm text-destructive">
      {text}
    </p>
  ) : null;
}

/** One settings section: a card with a title, a one-line description and actions on the right. */
export function Section({
  id,
  title,
  description,
  actions,
  children,
}: {
  id: string;
  title: string;
  description: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <Card aria-labelledby={`settings-${id}`} role="region">
      <CardHeader>
        <CardTitle id={`settings-${id}`} className="text-base">
          {title}
        </CardTitle>
        <CardDescription>{description}</CardDescription>
        {actions && <CardAction className="flex items-center gap-2">{actions}</CardAction>}
      </CardHeader>
      <CardContent className="flex flex-col gap-3">{children}</CardContent>
    </Card>
  );
}

/** Rows of label and value. */
export function KV({ rows }: { rows: [ReactNode, ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[1fr_auto] gap-x-4 gap-y-2 text-sm">
      {rows.map(([k, v], i) => (
        <div key={i} className="col-span-2 grid grid-cols-subgrid items-baseline border-b border-border/50 pb-2 last:border-0 last:pb-0">
          <dt className="min-w-0 text-muted-foreground">{k}</dt>
          <dd className="text-right tabular-nums">{v}</dd>
        </div>
      ))}
    </dl>
  );
}
