import { GlobeIcon, LinkIcon } from "lucide-react";
import { Generic } from "./Generic";
import { ResultOutput, str, ToolCard, type ToolProps } from "./parts";

/** The hostname of `url` without `www.`, or `null` for a bad URL. */
export function hostname(url: string): string | null {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return null;
  }
}

/** The distinct http(s) links in `text`, in order, at most `max`. */
export function links(text: string, max = 12): string[] {
  const out: string[] = [];
  for (const m of text.matchAll(/https?:\/\/[^\s<>()"'`\]]+/g)) {
    const u = m[0].replace(/[.,;:]+$/, "");
    if (!out.includes(u) && hostname(u)) out.push(u);
    if (out.length >= max) break;
  }
  return out;
}

/**
 * The Sources pattern: one chip per link with its hostname. No favicons; the
 * CSP allows no remote images.
 */
function Sources({ urls }: { urls: string[] }) {
  if (urls.length === 0) return null;
  return (
    <ul aria-label="Sources" className="flex flex-wrap gap-1.5">
      {urls.map((u) => (
        <li key={u}>
          <a
            href={u}
            target="_blank"
            rel="noreferrer noopener"
            title={u}
            className="inline-flex max-w-64 items-center gap-1 rounded-full border bg-background px-2 py-0.5 text-xs text-foreground transition-colors hover:bg-muted focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
          >
            <LinkIcon className="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
            <span className="truncate">{hostname(u)}</span>
          </a>
        </li>
      ))}
    </ul>
  );
}

/** WebFetch and WebSearch: what was fetched or asked, and the sources. */
export function Web(p: ToolProps) {
  const url = str(p.row.input, "url");
  const query = str(p.row.input, "query");
  if (!url && !query) return <Generic {...p} />;
  const urls = url ? [url] : links(p.row.result ?? "");
  return (
    <ToolCard
      row={p.row}
      icon={<GlobeIcon />}
      open={p.open}
      title={
        url ? (
          <>
            <span className="shrink-0 font-medium">Fetch</span>
            <span className="shrink-0 text-foreground">{hostname(url) ?? url}</span>
            <span className="min-w-0 truncate font-mono text-xs">{url.replace(/^https?:\/\/[^/]+/, "")}</span>
          </>
        ) : (
          <>
            <span className="shrink-0 font-medium">Search</span>
            <span className="min-w-0 truncate text-foreground">“{query}”</span>
          </>
        )
      }
      meta={!url && urls.length > 0 ? `${urls.length} ${urls.length === 1 ? "source" : "sources"}` : undefined}
    >
      {(p.row.result || urls.length > 0) && (
        <>
          <Sources urls={urls} />
          <ResultOutput {...p} />
        </>
      )}
    </ToolCard>
  );
}
