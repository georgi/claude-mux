import type { TokenUsage } from "@/api/types";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { cn } from "@/lib/utils";

/** The server reports cumulative usage across requests, including prior /clear sessions. */
function totalTokens(t: TokenUsage): number {
  return t.input + t.output + t.cache_read + t.cache_write;
}

const fmt = (n: number) => n.toLocaleString("en-US");
const short = (n: number) =>
  n >= 1_000_000 ? `${(n / 1_000_000).toFixed(1)}M` : n >= 1000 ? `${Math.round(n / 1000)}k` : String(n);

/** Cumulative token usage; these counters cannot measure current context occupancy. */
export function TokenUsageSummary({ tokens, className }: { tokens: TokenUsage; className?: string }) {
  const total = totalTokens(tokens);
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
            aria-label={`Cumulative token usage: ${fmt(total)} tokens`}
            className={cn("flex shrink-0 items-center gap-1.5 rounded-md px-1 text-xs text-muted-foreground", className)}
          />
        }
      >
        <span className="tabular-nums">{short(total)} tokens total</span>
      </HoverCardTrigger>
      <HoverCardContent align="end" className="w-56">
        <p className="mb-2 font-medium">Cumulative usage: {fmt(total)} tokens</p>
        <p className="mb-2 text-xs text-muted-foreground">Across all requests, including before /clear.</p>
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
