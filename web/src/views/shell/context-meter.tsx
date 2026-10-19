import type { TokenUsage } from "@/api/types";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { cn } from "@/lib/utils";

/** The context window the meter measures against. */
export const CONTEXT_WINDOW = 200_000;

/** Tokens in the context: everything the model read on its last turn. */
export function contextUsed(t: TokenUsage): number {
  return t.input + t.cache_read + t.cache_write;
}

/** Whole percent of the window in use, capped at 100. */
export function contextPercent(t: TokenUsage, window = CONTEXT_WINDOW): number {
  return Math.min(100, Math.round((contextUsed(t) / window) * 100));
}

const fmt = (n: number) => n.toLocaleString("en-US");
const short = (n: number) => (n >= 1000 ? `${Math.round(n / 1000)}k` : String(n));

/** How full the agent's context is: a thin bar and a percent; hover for the numbers. */
export function ContextMeter({ tokens, className }: { tokens: TokenUsage; className?: string }) {
  const pct = contextPercent(tokens);
  const tone = pct >= 90 ? "bg-status-failing" : pct >= 70 ? "bg-status-needs" : "bg-status-working";
  const rows: [string, number][] = [
    ["Input", tokens.input],
    ["Cache read", tokens.cache_read],
    ["Cache write", tokens.cache_write],
    ["Output", tokens.output],
  ];
  return (
    <HoverCard>
      <HoverCardTrigger
        render={
          <button
            type="button"
            className={cn("flex shrink-0 items-center gap-1.5 rounded-md px-1 text-xs text-muted-foreground", className)}
          />
        }
      >
        <span
          role="meter"
          aria-label="Context"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={pct}
          aria-valuetext={`${pct}% of ${short(CONTEXT_WINDOW)} context`}
          className="relative h-1.5 w-12 overflow-hidden rounded-full bg-muted"
        >
          <span className={cn("absolute inset-y-0 left-0 rounded-full", tone)} style={{ width: `${pct}%` }} />
        </span>
        <span className="tabular-nums" data-testid="context-percent">
          {pct}%
        </span>
      </HoverCardTrigger>
      <HoverCardContent align="end" className="w-56">
        <p className="mb-2 font-medium">
          Context {fmt(contextUsed(tokens))} / {fmt(CONTEXT_WINDOW)}
        </p>
        <dl className="grid grid-cols-[1fr_auto] gap-x-3 gap-y-1 text-xs">
          {rows.map(([k, n]) => (
            <div key={k} className="contents">
              <dt className="text-muted-foreground">{k}</dt>
              <dd className="text-right font-mono tabular-nums">{fmt(n)}</dd>
            </div>
          ))}
        </dl>
      </HoverCardContent>
    </HoverCard>
  );
}
