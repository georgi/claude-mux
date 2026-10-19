import { DownloadIcon, EllipsisVerticalIcon, LogOutIcon, PlusIcon, SettingsIcon } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
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
import type { Model } from "../../store";
import { ConnBadge } from "./conn-badge";

interface Props {
  m: Model;
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

export function Header({ m, seen, phone, wide, readOnly, needsCount, onNeeds, onNew, onSettings, onSignOut }: Props) {
  const install = useInstall();
  const iosHint = !install && isIos() && !isStandalone();
  const clients = m.presence.length;
  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
      <SidebarTrigger aria-label="Toggle the tree" />
      <Separator orientation="vertical" className="h-4" />
      <h1 className="min-w-0 truncate text-sm font-medium">{m.server?.name ?? "cmux"}</h1>
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
