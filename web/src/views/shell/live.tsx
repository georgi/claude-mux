import { ActivityIcon } from "lucide-react";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Item, ItemContent, ItemGroup, ItemMedia, ItemTitle, ItemDescription } from "@/components/ui/item";
import type { StateView } from "../../api/types";
import { agentStatus } from "@/lib/status";
import { StatusIcon } from "./status";

/** The phone's Live tab: agents running now, working ones first. */
export function Live({ view, onOpen }: { view: StateView; onOpen: (agent: string) => void }) {
  const live = view.agents.filter((a) => a.life === "running" || a.life === "starting");
  const rank = (a: (typeof live)[number]) => (a.activity === "needs_permission" ? 0 : a.activity === "working" ? 1 : 2);
  const plans = new Map(view.plans.map((p) => [p.id, p.title]));
  if (live.length === 0)
    return (
      <Empty>
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <ActivityIcon />
          </EmptyMedia>
          <EmptyTitle>No agent is running</EmptyTitle>
          <EmptyDescription>Running agents show up here.</EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  return (
    <ItemGroup className="gap-1 p-3">
      {[...live]
        .sort((a, b) => rank(a) - rank(b))
        .map((a) => (
          <Item
            key={a.id}
            size="xs"
            render={<button type="button" onClick={() => onOpen(a.id)} className="text-left" />}
            className="hover:bg-muted"
          >
            <ItemMedia>
              <StatusIcon status={agentStatus(a)} />
            </ItemMedia>
            <ItemContent>
              <ItemTitle className="break-all">{a.path}</ItemTitle>
              <ItemDescription>{plans.get(a.id.slice(0, a.id.indexOf("/"))) ?? ""}</ItemDescription>
            </ItemContent>
          </Item>
        ))}
    </ItemGroup>
  );
}
