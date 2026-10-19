import { ChevronLeftIcon, DownloadIcon, EllipsisVerticalIcon, LogOutIcon, PlusIcon, SettingsIcon } from "lucide-react";
import { Fragment } from "react";
import { toast } from "sonner";
import type { AgentView, StateView } from "@/api/types";
import { Badge } from "@/components/ui/badge";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Separator } from "@/components/ui/separator";
import { SidebarTrigger } from "@/components/ui/sidebar";
import { isIos, isStandalone, useInstall } from "../../pwa";
import { formatRoute, type Route } from "../../route";
import type { Model } from "../../store";
import { ConnBadge } from "./conn-badge";

/** One step of the breadcrumb: its text and where it goes. */
export interface Crumb {
  label: string;
  route: Route;
}

/** The sub-agent's own name: its path after the task id. */
function subName(a: AgentView): string {
  return a.path.slice(a.path.indexOf("/") + 1);
}

/** Plan › task › sub-agent for an agent, the plan for a board, the title for a terminal. */
export function crumbsFor(v: StateView | null, route: Route): Crumb[] {
  if (!v) return [];
  const planCrumb = (id: string): Crumb[] => {
    const p = v.plans.find((x) => x.id === id);
    return p ? [{ label: p.title, route: { view: "board", plan: p.id } }] : [];
  };
  switch (route.view) {
    case "agent": {
      const a = v.agents.find((x) => x.id === route.agent);
      if (!a) return [];
      const plan = v.plans.find((p) => p.agents.includes(a.id) || a.id.startsWith(`${p.id}/`));
      const out = plan ? planCrumb(plan.id) : [];
      if (a.level === "executor") return [...out, { label: "Executor", route: { view: "agent", agent: a.id, mode: null } }];
      const t = v.tasks.find((x) => x.id === a.task && (!plan || x.plan === plan.id));
      const root = a.level === "sub" ? a.parent : a.id;
      if (root)
        out.push({ label: t ? `${t.id} ${t.title}` : (a.task ?? a.path), route: { view: "agent", agent: root, mode: null } });
      if (a.level === "sub") out.push({ label: subName(a), route: { view: "agent", agent: a.id, mode: null } });
      return out;
    }
    case "board":
      return planCrumb(route.plan);
    case "term": {
      const t = v.terminals.find((x) => x.id === route.term);
      return [{ label: t?.title ?? `terminal ${route.term}`, route }];
    }
    default:
      return [];
  }
}

/** Every crumb but the current page links; a phone shows the last one and a back button to its parent. */
export function Crumbs({ crumbs, phone }: { crumbs: Crumb[]; phone: boolean }) {
  if (crumbs.length === 0) return null;
  const last = crumbs[crumbs.length - 1];
  const parent = crumbs[crumbs.length - 2];
  const shown = phone ? [last] : crumbs;
  return (
    <Breadcrumb className="min-w-0">
      <BreadcrumbList className="flex-nowrap">
        {phone && parent && (
          <BreadcrumbItem>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={`Back to ${parent.label}`}
              render={<a href={formatRoute(parent.route)} />}
              nativeButton={false}
            >
              <ChevronLeftIcon />
            </Button>
          </BreadcrumbItem>
        )}
        {shown.map((c, i) => (
          <Fragment key={formatRoute(c.route)}>
            {i > 0 && <BreadcrumbSeparator />}
            <BreadcrumbItem className="min-w-0">
              {c === last ? (
                <BreadcrumbPage className="truncate">{c.label}</BreadcrumbPage>
              ) : (
                <BreadcrumbLink href={formatRoute(c.route)} className="max-w-48 truncate">
                  {c.label}
                </BreadcrumbLink>
              )}
            </BreadcrumbItem>
          </Fragment>
        ))}
      </BreadcrumbList>
    </Breadcrumb>
  );
}

interface Props {
  m: Model;
  crumbs: Crumb[];
  seen: boolean;
  phone: boolean;
  wide: boolean;
  readOnly: boolean;
  needsCount: number;
  onNeeds: () => void;
  onNew: () => void;
  onSettings: () => void;
  onSignOut: () => void;
}

export function Header({ m, crumbs, seen, phone, wide, readOnly, needsCount, onNeeds, onNew, onSettings, onSignOut }: Props) {
  const install = useInstall();
  const iosHint = !install && isIos() && !isStandalone();
  const clients = m.presence.length;
  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
      <SidebarTrigger aria-label="Toggle the tree" />
      <Separator orientation="vertical" className="h-4" />
      <h1 className={crumbs.length ? "sr-only" : "min-w-0 truncate text-sm font-medium"}>{m.server?.name ?? "cmux"}</h1>
      <Crumbs crumbs={crumbs} phone={phone} />
      <ConnBadge status={m.conn} seen={seen} />
      {!phone && m.conn === "open" && (
        <span className="text-xs text-muted-foreground">
          {clients} client{clients === 1 ? "" : "s"}
        </span>
      )}
      {readOnly && <Badge variant="outline">Read-only</Badge>}
      <span className="grow" />
      {!phone && !wide && (
        <Button variant="ghost" size="sm" onClick={onNeeds}>
          Needs you
          {needsCount > 0 && <Badge>{needsCount}</Badge>}
        </Button>
      )}
      {!phone && (
        <Button variant="outline" size="sm" onClick={onNew}>
          <PlusIcon data-icon="inline-start" />
          New plan
        </Button>
      )}
      {install && (
        <Button variant="outline" size="sm" onClick={() => void install()}>
          <DownloadIcon data-icon="inline-start" />
          Install
        </Button>
      )}
      <DropdownMenu>
        <DropdownMenuTrigger
          render={<Button variant="ghost" size="icon" aria-label="Menu" />}
        >
          <EllipsisVerticalIcon />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem onClick={onSettings}>
            <SettingsIcon />
            Settings
          </DropdownMenuItem>
          {iosHint && (
            <DropdownMenuItem
              onClick={() => toast("Tap Share, then Add to Home Screen, to install cmux.")}
            >
              <DownloadIcon />
              Install app
            </DropdownMenuItem>
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" onClick={onSignOut}>
            <LogOutIcon />
            Sign out
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </header>
  );
}
