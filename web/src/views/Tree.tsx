import { FolderTreeIcon } from "lucide-react";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import {
  SidebarGroup,
  SidebarGroupContent,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@/components/ui/sidebar";
import type { StateView } from "../api/types";
import { useSpinner } from "../hooks";
import { buildTree, type Row } from "../tree";
import { StatusIcon } from "./shell/status";

interface Props {
  view: StateView;
  /** The open row's key: an agent id or `term-<id>`. */
  selected: string | null;
  onOpen: (row: Row) => void;
  /** Opens a plan's board from its row. */
  onPlan: (plan: string) => void;
}

/** The plan/agent/terminal tree in the sidebar (UI design 4.1, left). Must sit inside a SidebarProvider. */
export function Tree({ view, selected, onOpen, onPlan }: Props) {
  const spin = useSpinner(view.agents.some((a) => a.activity === "working"));
  const { setOpenMobile } = useSidebar();
  const rows = buildTree(view, spin);
  if (rows.length === 0)
    return (
      <Empty className="p-4">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <FolderTreeIcon />
          </EmptyMedia>
          <EmptyTitle>Nothing yet</EmptyTitle>
          <EmptyDescription>No plans or terminals yet.</EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  return (
    <SidebarGroup>
      <SidebarGroupContent>
        <SidebarMenu>
          {rows.map((r) => {
            const isPlan = r.depth === 0 && r.agent === undefined && r.term === undefined;
            const go = () => {
              if (isPlan) onPlan(r.key);
              else onOpen(r);
              setOpenMobile(false);
            };
            return (
              <SidebarMenuItem key={r.key}>
                <SidebarMenuButton
                  isActive={r.key === selected}
                  onClick={go}
                  aria-current={r.key === selected ? "page" : undefined}
                  title={r.agent ?? (isPlan ? "Open the board" : r.label)}
                  className={r.dim ? "opacity-60" : undefined}
                  style={{ paddingLeft: `${0.5 + r.depth * 0.75}rem` }}
                >
                  <StatusIcon status={r.status} />
                  <span className={isPlan ? "truncate font-medium" : "truncate"}>{r.label}</span>
                  {r.detail && <span className="ml-auto shrink-0 text-xs text-muted-foreground">{r.detail}</span>}
                </SidebarMenuButton>
              </SidebarMenuItem>
            );
          })}
        </SidebarMenu>
      </SidebarGroupContent>
    </SidebarGroup>
  );
}
