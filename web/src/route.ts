// Where the app is, kept in the URL hash so reload and back work:
// `#/agent/<id>/chat`, `#/term/3`, `#/board/<plan>`, `#/new`, `#/settings`,
// and the phone tabs `#/needs`, `#/agents`, `#/plans`, `#/live`.

export type AgentMode = "terminal" | "chat" | "split";

export type Route =
  | { view: "home" }
  | { view: "agent"; agent: string; mode: AgentMode | null }
  | { view: "term"; term: number }
  | { view: "board"; plan: string }
  | { view: "new" }
  | { view: "settings" }
  | { view: "needs" }
  | { view: "agents" }
  | { view: "plans" }
  | { view: "live" };

export function parseRoute(hash: string): Route {
  const parts = hash.replace(/^#\/?/, "").split("/").map(decodeURIComponent);
  switch (parts[0]) {
    case "agent": {
      // Agent ids hold a `/`: everything up to an optional mode is the id.
      const last = parts[parts.length - 1];
      const mode = last === "chat" || last === "terminal" || last === "split" ? last : null;
      const id = parts.slice(1, mode ? -1 : undefined).join("/");
      return id ? { view: "agent", agent: id, mode } : { view: "home" };
    }
    case "term": {
      const n = Number(parts[1]);
      return Number.isInteger(n) && parts[1] !== "" ? { view: "term", term: n } : { view: "home" };
    }
    case "board":
      return parts[1] ? { view: "board", plan: parts[1] } : { view: "plans" };
    case "new":
    case "settings":
    case "needs":
    case "agents":
    case "plans":
    case "live":
      return { view: parts[0] };
    default:
      return { view: "home" };
  }
}

export function formatRoute(r: Route): string {
  switch (r.view) {
    case "home":
      return "#/";
    case "agent":
      return `#/agent/${r.agent.split("/").map(encodeURIComponent).join("/")}${r.mode ? `/${r.mode}` : ""}`;
    case "term":
      return `#/term/${r.term}`;
    case "board":
      return `#/board/${encodeURIComponent(r.plan)}`;
    default:
      return `#/${r.view}`;
  }
}
