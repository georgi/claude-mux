import { ChevronRightIcon, EllipsisIcon, FolderTreeIcon, SearchIcon } from "lucide-react";
import { type FocusEvent, type KeyboardEvent, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from "@/components/ui/context-menu";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { Kbd } from "@/components/ui/kbd";
import { Progress } from "@/components/ui/progress";
import { SidebarInput, useSidebar } from "@/components/ui/sidebar";
import { ciStatus } from "@/lib/status";
import { relativeTime, useNow } from "@/lib/time";
import { cn } from "@/lib/utils";
import type { Api } from "../api/http";
import type { StateView, TermId } from "../api/types";
import type { AgentMode } from "../route";
import {
  buildTree,
  expandedByDefault,
  filterTree,
  type RowActionId,
  rowActions,
  rowPath,
  type TreeNode,
  visibleRows,
} from "../tree";
import { StatusDot, StatusIcon } from "./shell/status";

interface Props {
  view: StateView;
  /** The open row's key: an agent id or `term-<id>`. */
  selected: string | null;
  api: Api;
  readOnly?: boolean;
  onOpenAgent: (agent: string, mode: AgentMode | null) => void;
  onOpenTerm: (term: TermId) => void;
  /** Opens a plan's board. */
  onPlan: (plan: string) => void;
}

const HOVER_DELAY = 400;

function tokens(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

/** The plan/agent/terminal tree in the sidebar (UI design 4.1, left). Must sit inside a SidebarProvider. */
export function Tree({ view, selected, api, readOnly = false, onOpenAgent, onOpenTerm, onPlan }: Props) {
  const { isMobile, setOpenMobile } = useSidebar();
  const now = useNow();
  const [query, setQuery] = useState("");
  const [toggled, setToggled] = useState<Record<string, boolean>>({});
  const [focus, setFocus] = useState<string | null>(null);
  const [menu, setMenu] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const items = useRef(new Map<string, HTMLLIElement>());

  const all = useMemo(() => buildTree(view), [view]);
  const nodes = useMemo(() => filterTree(all, query), [all, query]);
  const filtering = query.trim() !== "";
  const isOpen = (n: TreeNode) => filtering || (toggled[n.key] ?? expandedByDefault(n));
  const rows = visibleRows(nodes, isOpen);
  const current = rows.find((r) => r.node.key === focus) ?? rows.find((r) => r.node.key === selected) ?? rows[0];

  // `/` anywhere in the sidebar jumps to the filter.
  useEffect(() => {
    const root = rootRef.current;
    const scope = root?.closest<HTMLElement>('[data-slot="sidebar"]') ?? root;
    if (!scope) return;
    const onKey = (e: globalThis.KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (e.key !== "/" || e.metaKey || e.ctrlKey || e.altKey) return;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      e.preventDefault();
      filterRef.current?.focus();
    };
    scope.addEventListener("keydown", onKey);
    return () => scope.removeEventListener("keydown", onKey);
  }, []);

  const toggle = (n: TreeNode, open: boolean) => setToggled((t) => ({ ...t, [n.key]: open }));
  const moveTo = (key: string | undefined) => {
    if (!key) return;
    setFocus(key);
    items.current.get(key)?.focus();
  };

  const open = (n: TreeNode) => {
    if (n.kind === "group") return toggle(n, !isOpen(n));
    if (n.agent) onOpenAgent(n.agent.id, null);
    else if (n.term !== undefined) onOpenTerm(n.term);
    else if (n.plan) onPlan(n.plan.id);
    else if (n.task) onPlan(n.task.plan);
    setOpenMobile(false);
  };

  const copy = (text: string, what: string) =>
    navigator.clipboard?.writeText(text).then(
      () => toast(`Copied the ${what}`),
      () => toast.error(`Could not copy the ${what}`),
    );

  const act = (n: TreeNode, id: RowActionId) => {
    const a = n.agent;
    switch (id) {
      case "chat":
        if (a) onOpenAgent(a.id, "chat");
        break;
      case "terminal":
        if (a) onOpenAgent(a.id, "terminal");
        else if (n.term !== undefined) onOpenTerm(n.term);
        break;
      case "board":
        onPlan(n.plan?.id ?? n.task?.plan ?? "");
        break;
      case "stop":
      case "resume":
        if (a)
          (id === "stop" ? api.stopAgent(a.id) : api.resumeAgent(a.id)).then(
            () => toast(`${id === "stop" ? "Stopped" : "Resumed"} ${a.path}`),
            (e: Error) => toast.error(e.message),
          );
        return;
      case "copy_branch":
        if (n.task?.branch) copy(n.task.branch, "branch");
        return;
      case "copy_path": {
        const p = rowPath(n);
        if (p) copy(p, "path");
        return;
      }
      case "open_pr":
        if (n.task?.pr) window.open(n.task.pr.url, "_blank", "noopener");
        return;
    }
    setOpenMobile(false);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLUListElement>) => {
    const target = e.target as HTMLElement;
    if (target.getAttribute("role") !== "treeitem") return;
    const i = rows.findIndex((r) => r.node.key === target.dataset.key);
    if (i < 0) return;
    const cur = rows[i];
    const n = cur.node;
    const kids = n.children.length > 0;
    switch (e.key) {
      case "ArrowDown":
        moveTo(rows[i + 1]?.node.key);
        break;
      case "ArrowUp":
        moveTo(rows[i - 1]?.node.key);
        break;
      case "Home":
        moveTo(rows[0]?.node.key);
        break;
      case "End":
        moveTo(rows[rows.length - 1]?.node.key);
        break;
      case "ArrowRight":
        if (!kids) break;
        if (!isOpen(n)) toggle(n, true);
        else moveTo(rows[i + 1]?.node.key);
        break;
      case "ArrowLeft":
        if (kids && isOpen(n) && !filtering) toggle(n, false);
        else moveTo(cur.parent ?? undefined);
        break;
      case "Enter":
        open(n);
        break;
      case "ContextMenu":
        setMenu(n.key);
        break;
      case "F10":
        if (!e.shiftKey) return;
        setMenu(n.key);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const onFilterKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      if (query) setQuery("");
      else moveTo(current?.node.key);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      moveTo(rows[0]?.node.key);
    }
  };

  if (all.length === 0)
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

  const render = (n: TreeNode, level: number): ReactNode => {
    const kids = n.children.length > 0;
    const expanded = kids && isOpen(n);
    const actions = rowActions(n, readOnly);
    const item = {
      role: "treeitem",
      "aria-level": level,
      "aria-expanded": kids ? expanded : undefined,
      "aria-selected": n.key === selected,
      "aria-label": n.label,
      tabIndex: current?.node.key === n.key ? 0 : -1,
      "data-key": n.key,
      ref: (el: HTMLLIElement | null) => {
        if (el) items.current.set(n.key, el);
        else items.current.delete(n.key);
      },
      onFocus: (e: FocusEvent<HTMLLIElement>) => {
        if (e.target === e.currentTarget) setFocus(n.key);
      },
      className: "group/item outline-none",
    } as const;
    const label = (
      <span
        className={cn(
          "min-w-0 truncate",
          n.kind === "plan" || n.kind === "group" ? "font-medium" : undefined,
          n.struck && "text-muted-foreground line-through",
        )}
      >
        {n.label}
      </span>
    );
    const row = (
      <ContextMenu>
        <ContextMenuTrigger
          className={cn(
            "flex h-8 cursor-pointer items-center gap-2 rounded-md pr-1 text-sm text-sidebar-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
            "group-focus-visible/item:ring-2 group-focus-visible/item:ring-sidebar-ring",
            n.key === selected && "bg-sidebar-accent font-medium text-sidebar-accent-foreground",
            n.dim && "opacity-60",
          )}
          style={{ paddingLeft: `${0.25 + (level - 1) * 0.75}rem` }}
          onClick={() => {
            setFocus(n.key);
            open(n);
          }}
          title={n.agent?.id ?? (n.kind === "plan" ? "Open the board" : n.label)}
        >
          {kids ? (
            <CollapsibleTrigger
              tabIndex={-1}
              aria-label={expanded ? `Collapse ${n.label}` : `Expand ${n.label}`}
              className="-mr-1 flex size-4 shrink-0 items-center justify-center rounded-sm text-muted-foreground hover:text-foreground"
              onClick={(e) => e.stopPropagation()}
            >
              <ChevronRightIcon
                aria-hidden
                className={cn("size-3.5 transition-transform duration-(--duration-fast)", expanded && "rotate-90")}
              />
            </CollapsibleTrigger>
          ) : (
            <span aria-hidden className="-mr-1 size-4 shrink-0" />
          )}
          {n.kind === "group" ? null : <StatusDot status={n.status} />}
          {!isMobile && (n.agent || n.task) ? <Preview node={n} now={now} trigger={label} /> : label}
          {n.stalled && (
            <span aria-hidden className="shrink-0 text-xs text-status-stalled">
              ⚠
            </span>
          )}
          <span className="ml-auto flex shrink-0 items-center gap-1.5">
            {n.badge && (
              <span className="rounded-sm border px-1 text-[0.625rem] leading-4 text-muted-foreground">{n.badge}</span>
            )}
            {n.detail && <span className="text-xs text-muted-foreground tabular-nums">{n.detail}</span>}
            {n.needs > 0 && (
              <span
                aria-label={`${n.needs} need you`}
                className="min-w-4 rounded-full bg-status-needs px-1 text-center text-[0.625rem] leading-4 font-medium text-white tabular-nums"
              >
                {n.needs}
              </span>
            )}
            {actions.length > 0 && (
              <DropdownMenu open={menu === n.key} onOpenChange={(o) => setMenu(o ? n.key : null)}>
                <DropdownMenuTrigger
                  tabIndex={-1}
                  aria-label={`Actions for ${n.label}`}
                  className="flex size-5 items-center justify-center rounded-sm text-muted-foreground opacity-100 hover:bg-sidebar-accent hover:text-foreground md:opacity-0 md:group-hover/item:opacity-100 md:group-focus-visible/item:opacity-100 md:data-popup-open:opacity-100"
                  onClick={(e) => e.stopPropagation()}
                >
                  <EllipsisIcon aria-hidden className="size-4" />
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  align="end"
                  className="w-auto"
                  finalFocus={{ current: items.current.get(n.key) ?? null }}
                >
                  {actions.map((x) => (
                    <DropdownMenuItem key={x.id} onClick={() => act(n, x.id)}>
                      {x.label}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            )}
          </span>
        </ContextMenuTrigger>
        {actions.length > 0 && (
          <ContextMenuContent>
            {actions.map((x) => (
              <ContextMenuItem key={x.id} onClick={() => act(n, x.id)}>
                {x.label}
              </ContextMenuItem>
            ))}
          </ContextMenuContent>
        )}
      </ContextMenu>
    );
    if (!kids) return <li key={n.key} {...item}>{row}</li>;
    return (
      <Collapsible
        key={n.key}
        open={expanded}
        onOpenChange={(o) => !filtering && toggle(n, o)}
        render={<li {...item} />}
      >
        {row}
        <CollapsibleContent render={<ul role="group" />}>{n.children.map((c) => render(c, level + 1))}</CollapsibleContent>
      </Collapsible>
    );
  };

  return (
    <div ref={rootRef} className="flex flex-col gap-1 p-2">
      <div className="relative">
        <SearchIcon aria-hidden className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <SidebarInput
          ref={filterRef}
          type="search"
          aria-label="Filter the tree"
          placeholder="Filter"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onFilterKey}
          className="pr-8 pl-7"
        />
        <Kbd className="absolute top-1/2 right-1.5 -translate-y-1/2">/</Kbd>
      </div>
      {rows.length === 0 ? (
        <p className="px-2 py-3 text-sm text-muted-foreground">No matches.</p>
      ) : (
        <ul role="tree" aria-label="Agents and plans" onKeyDown={onKeyDown} className="flex flex-col gap-px">
          {nodes.map((n) => render(n, 1))}
        </ul>
      )}
    </div>
  );
}

/** The hover preview of an agent or task row (desktop only). */
function Preview({ node, now, trigger }: { node: TreeNode; now: number; trigger: ReactNode }) {
  const { agent: a, task: t } = node;
  const used = a ? a.tokens.input + a.tokens.output + a.tokens.cache_read + a.tokens.cache_write : 0;
  return (
    <HoverCard>
      <HoverCardTrigger delay={HOVER_DELAY} render={<span className="flex min-w-0" />}>
        {trigger}
      </HoverCardTrigger>
      <HoverCardContent side="right" align="start" className="flex w-72 flex-col gap-2">
        <div className="flex items-center gap-2">
          <StatusIcon status={node.status} />
          <span className="font-medium">{t?.title ?? a?.path}</span>
        </div>
        {t?.branch && <div className="truncate font-mono text-xs text-muted-foreground">{t.branch}</div>}
        {t?.pr && (
          <div className="flex items-center gap-1.5 text-xs">
            PR #{t.pr.number}
            {t.pr.ci && (
              <>
                <StatusIcon status={ciStatus(t.pr.ci)} />
                <span className="text-muted-foreground">{ciStatus(t.pr.ci).label}</span>
              </>
            )}
          </div>
        )}
        {t && t.criteria_total > 0 && (
          <Progress
            value={Math.round((t.criteria_done / t.criteria_total) * 100)}
            aria-label={`${t.criteria_done} of ${t.criteria_total} criteria`}
            className="gap-1 text-xs"
          >
            <span className="text-muted-foreground">
              Criteria {t.criteria_done}/{t.criteria_total}
            </span>
          </Progress>
        )}
        {a && (
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 text-xs">
            {a.idle_since !== null && (
              <>
                <dt className="text-muted-foreground">Idle since</dt>
                <dd>{relativeTime(a.idle_since, now)}</dd>
              </>
            )}
            <dt className="text-muted-foreground">Tokens</dt>
            <dd className="tabular-nums">{tokens(used)}</dd>
          </dl>
        )}
        {a?.report && <p className="line-clamp-3 text-xs text-muted-foreground">{a.report.summary}</p>}
      </HoverCardContent>
    </HoverCard>
  );
}
