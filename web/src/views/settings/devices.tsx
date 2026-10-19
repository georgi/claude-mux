import { TrashIcon } from "lucide-react";
import { useState } from "react";
import type { Api } from "../../api/http";
import type { DeviceView } from "../../api/types";
import { Err, SaveState, Section, useLoad, useSave } from "./common";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from "@/components/ui/item";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { relativeTime, useNow } from "@/lib/time";

const full = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

/** "3h ago" with the full date in a tooltip. */
function Ago({ ms, now }: { ms: number; now: number }) {
  const rel = relativeTime(ms, now);
  const text = rel === "now" ? "just now" : /^\d/.test(rel) ? `${rel} ago` : `on ${rel}`;
  return (
    <Tooltip>
      <TooltipTrigger render={<time dateTime={new Date(ms).toISOString()} className="underline decoration-dotted underline-offset-2" />}>
        {text}
      </TooltipTrigger>
      <TooltipContent>{full.format(ms)}</TooltipContent>
    </Tooltip>
  );
}

export function Devices({ api, readOnly, onSignedOut }: { api: Api; readOnly?: boolean; onSignedOut: () => void }) {
  const [resp, error, reload] = useLoad(() => api.devices(), [api]);
  const [target, setTarget] = useState<DeviceView | null>(null);
  const [save, run] = useSave();
  const now = useNow();
  // This device first, then the most recently seen.
  const devices = resp?.devices.slice().sort((a, b) => Number(b.current) - Number(a.current) || (b.last_seen ?? 0) - (a.last_seen ?? 0));
  const revoke = async (d: DeviceView) => {
    setTarget(null);
    if (await run(() => api.revokeDevice(d.id))) {
      if (d.current) onSignedOut();
      else reload();
    }
  };
  return (
    <Section id="devices" title="Devices" description="Paired browsers. Pair a new one with `cmuxd pair` on the server." actions={<SaveState save={save} />}>
      <Err text={error} />
      {!resp && !error && <Skeleton className="h-14 w-full" />}
      {devices?.length === 0 && <p className="text-sm text-muted-foreground">No paired devices.</p>}
      <ItemGroup className="gap-2">
        {devices?.map((d) => (
          <Item key={d.id} variant="outline" size="sm">
            <ItemContent>
              <ItemTitle className="flex-wrap">
                {d.name}
                {d.current && <Badge>This device</Badge>}
                <Badge variant="outline">{d.scope === "read" ? "Read only" : "Full access"}</Badge>
              </ItemTitle>
              <ItemDescription>
                Added <Ago ms={d.created} now={now} /> · last seen {d.last_seen ? <Ago ms={d.last_seen} now={now} /> : "never"}
              </ItemDescription>
            </ItemContent>
            <ItemActions>
              <Button
                variant="ghost"
                size="sm"
                className="text-destructive"
                disabled={readOnly || save.state === "busy"}
                onClick={() => setTarget(d)}
                aria-label={`Revoke ${d.name}`}
              >
                <TrashIcon /> Revoke
              </Button>
            </ItemActions>
          </Item>
        ))}
      </ItemGroup>
      <AlertDialog open={target !== null} onOpenChange={(o) => !o && setTarget(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Revoke “{target?.name}”?</AlertDialogTitle>
            <AlertDialogDescription>
              {target?.current
                ? "This is the device you use now. You will be signed out and must pair it again."
                : "The device loses access now. It must pair again to connect."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={() => target && void revoke(target)}>
              Revoke {target?.current ? "and sign out" : "device"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Section>
  );
}
