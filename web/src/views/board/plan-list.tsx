import { ClipboardListIcon, PlusIcon } from "lucide-react";
import type { PlanView, StateView } from "../../api/types";
import { Button } from "@/components/ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from "@/components/ui/item";
import { Progress } from "@/components/ui/progress";
import { PlanBadge } from "./state";

/** The phone's Plans tab: every plan, newest first. */
export function PlanList({ view, onOpen, onNew }: { view: StateView; onOpen: (plan: string) => void; onNew: () => void }) {
  const plans = [...view.plans].sort((a, b) => (a.id < b.id ? 1 : -1));
  const progress = (p: PlanView) => {
    const ts = view.tasks.filter((t) => t.plan === p.id);
    const m = ts.filter((t) => t.state === "merged").length;
    return { m, n: ts.length };
  };
  return (
    <section className="mx-auto flex w-full max-w-2xl flex-col gap-3 p-4">
      {plans.length === 0 ? (
        <Empty>
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <ClipboardListIcon />
            </EmptyMedia>
            <EmptyTitle>No plans yet</EmptyTitle>
            <EmptyDescription>Create a plan to start a team of agents.</EmptyDescription>
          </EmptyHeader>
          <EmptyContent>
            <Button onClick={onNew}>
              <PlusIcon /> New plan
            </Button>
          </EmptyContent>
        </Empty>
      ) : (
        <ItemGroup className="gap-2">
          {plans.map((p) => {
            const { m, n } = progress(p);
            return (
              <div key={p.id} role="listitem">
                <Item variant="outline" render={<button type="button" onClick={() => onOpen(p.id)} className="cursor-pointer text-left hover:bg-muted" />}>
                  <ItemContent>
                    <ItemTitle>{p.title}</ItemTitle>
                    <ItemDescription>
                      {m}/{n} merged
                    </ItemDescription>
                    <Progress value={n ? Math.round((m / n) * 100) : 0} aria-label="Plan progress" className="w-full" />
                  </ItemContent>
                  <ItemActions>
                    <PlanBadge state={p.state} paused={p.paused} />
                  </ItemActions>
                </Item>
              </div>
            );
          })}
        </ItemGroup>
      )}
      {plans.length > 0 && (
        <Button className="self-start" onClick={onNew}>
          <PlusIcon /> New plan
        </Button>
      )}
    </section>
  );
}
