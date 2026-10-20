import { BotIcon, ExternalLinkIcon } from "lucide-react";
import type { AgentView } from "@/api/types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { agentStatus } from "@/lib/status";
import { cn } from "@/lib/utils";
import { Markdown } from "../../lazy";
import { Generic } from "./Generic";
import { FullOutputButton, SPAWN_SUBAGENT, str, ToolCard, useFullResult, type ToolProps } from "./parts";

/** A sub-agent's short name: its path after the task (`T-0003/lint` → `lint`). */
const short = (a: AgentView) => a.path.slice(a.path.lastIndexOf("/") + 1);

/** `"name": "lint"` in the one-line summary of a `spawn_subagent` call. */
function spawnName(summary: string): string | undefined {
  return /"name"\s*:\s*"([^"]+)"/.exec(summary)?.[1] ?? /^name[=:]\s*(\S+)/.exec(summary)?.[1];
}

/**
 * The sub-agent of `parent` this call started: the one named in the call,
 * else the one whose name the description mentions.
 */
export function subagentFor(agents: AgentView[], parent: string, name: string | undefined, text: string): AgentView | undefined {
  const kids = agents.filter((a) => a.parent === parent && a.level === "sub");
  if (name) return kids.find((a) => short(a) === name);
  const t = text.toLowerCase();
  return kids.find((a) => new RegExp(`\\b${short(a).toLowerCase().replace(/[^\w-]/g, "")}\\b`).test(t));
}

/** The sub-agent's live status, as the tree shows it. */
function Activity({ agent }: { agent: AgentView }) {
  const s = agentStatus(agent);
  return (
    <span className="inline-flex items-center gap-1.5 text-xs">
      <span
        aria-hidden="true"
        className={cn("size-2 rounded-full", s.token === "working" && "animate-pulse")}
        style={{ background: `var(--status-${s.token})` }}
      />
      {s.label}
    </span>
  );
}

/** The Subagent pattern: what it was asked, its type, its live state and its report. */
export function Task(p: ToolProps) {
  const spawn = p.row.name === SPAWN_SUBAGENT;
  const description = spawn ? spawnName(p.row.summary) : str(p.row.input, "description");
  const full = useFullResult(p);
  if (!description) return <Generic {...p} />;
  const type = spawn ? "cmux" : str(p.row.input, "subagent_type");
  const sub = p.agents ? subagentFor(p.agents, p.agent, spawn ? description : undefined, `${description} ${p.row.summary}`) : undefined;
  const open = sub && p.onOpenAgent;
  return (
    <ToolCard
      row={p.row}
      icon={<BotIcon />}
      open={p.open}
      title={
        <>
          <span className="min-w-0 truncate font-medium">{description}</span>
          {type && (
            <Badge variant="outline" className="h-4 shrink-0 px-1.5 font-mono text-[0.65rem]">
              {type}
            </Badge>
          )}
        </>
      }
      meta={sub && <Activity agent={sub} />}
      action={
        open && (
          <Button size="xs" variant="ghost" aria-label={`Open agent ${short(sub)}`} onClick={() => p.onOpenAgent!(sub.id)}>
            <ExternalLinkIcon /> Open
          </Button>
        )
      }
    >
      {p.row.result ? (
        <div className="flex flex-col gap-2 rounded-lg border bg-muted/30 p-3 text-sm text-foreground">
          <Markdown>{full.text}</Markdown>
          <FullOutputButton full={full} total={p.row.resultLen} />
        </div>
      ) : null}
    </ToolCard>
  );
}
