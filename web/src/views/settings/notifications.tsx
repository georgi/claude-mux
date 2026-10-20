import { useEffect, useState } from "react";
import type { Api } from "../../api/http";
import type { NotifyKind } from "../../api/types";
import { disablePush, enablePush, kindsOf, loadNotify, NOTIFY_KINDS, type PushState, pushState, saveNotify, updateKinds } from "../../push";
import { isIos, isStandalone } from "../../pwa";
import { SaveState, Section, useSave } from "./common";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";

const KIND_TEXT: Record<NotifyKind, string> = {
  permission: "An agent waits for you to allow a tool.",
  question: "An agent asks you a question.",
  plan_review: "A planner finished a plan for you to accept.",
  stalled: "An agent has had no output for a long time, or exited.",
  ci_failing: "CI fails on a task's pull request.",
  plan_done: "Every task of a plan is merged.",
};

const STATE_LABEL: Record<PushState, string> = {
  on: "On",
  off: "Off",
  denied: "Blocked",
  insecure: "Needs HTTPS",
  unsupported: "Not available",
};

type Browser = "ios" | "safari" | "firefox" | "chromium";

export function browserOf(ua = navigator.userAgent): Browser {
  if (isIos(ua)) return "ios";
  if (/Firefox\//.test(ua)) return "firefox";
  if (/Safari\//.test(ua) && !/Chrom(e|ium)\/|Edg\//.test(ua)) return "safari";
  return "chromium";
}

/** What to do about a push state, for this browser. */
export function pushHint(state: PushState, browser: Browser, standalone: boolean): string {
  switch (state) {
    case "on":
      return "This device gets a push notification for the kinds turned on above.";
    case "off":
      return "Turn on push to be notified when the app is closed.";
    case "insecure":
      return "Web Push needs HTTPS. Open cmux through its https:// address, for example with Tailscale Serve.";
    case "unsupported":
      if (browser === "ios" && !standalone) return "On iPhone and iPad, push works only in the installed app. Tap Share → Add to Home Screen, then open cmux from the home screen.";
      return "This browser has no Web Push. Use a current Chrome, Edge, Firefox or Safari.";
    case "denied":
      if (browser === "ios") return "Open the iOS Settings → Notifications → cmux, and turn on Allow Notifications.";
      if (browser === "safari") return "Open Safari → Settings → Websites → Notifications, and set this site to Allow.";
      if (browser === "firefox") return "Click the permissions icon in the address bar, clear the blocked notifications, then reload.";
      return "Click the site icon left of the address, set Notifications to Allow, then reload.";
  }
}

export function Notifications({ api }: { api: Api }) {
  const [on, setOn] = useState(loadNotify);
  const [push, setPush] = useState<PushState | null>(null);
  const [save, run] = useSave();
  const refresh = () => pushState().then(setPush, () => setPush("unsupported"));
  useEffect(() => {
    void refresh();
  }, []);
  const flip = (k: string, value: boolean) => {
    const next = { ...on, [k]: value };
    setOn(next);
    void run(async () => {
      saveNotify(next);
      if (push === "on") await updateKinds(api, kindsOf(next));
    });
  };
  const toggle = (f: () => Promise<unknown>) => void run(f).finally(refresh);
  const action =
    push === "off" ? (
      <Button size="sm" disabled={save.state === "busy"} onClick={() => toggle(() => enablePush(api, kindsOf(on)))}>
        Enable push
      </Button>
    ) : push === "on" ? (
      <Button size="sm" variant="outline" disabled={save.state === "busy"} onClick={() => toggle(() => disablePush(api))}>
        Disable push
      </Button>
    ) : null;
  return (
    <Section id="notifications" title="Notifications" description="What notifies this device, in the app and by push." actions={action}>
      <div className="flex flex-col divide-y divide-border/50">
        {NOTIFY_KINDS.map(([k, label]) => (
          <div key={k} className="flex items-center justify-between gap-4 py-2.5 first:pt-0 last:pb-0">
            <div className="flex min-w-0 flex-col gap-0.5">
              <Label htmlFor={`notify-${k}`}>{label}</Label>
              <p id={`notify-${k}-text`} className="text-xs text-muted-foreground">
                {KIND_TEXT[k]}
              </p>
            </div>
            <Switch id={`notify-${k}`} aria-describedby={`notify-${k}-text`} checked={on[k] !== false} onCheckedChange={(v) => flip(k, v)} />
          </div>
        ))}
      </div>
      {push && (
        <div className="flex flex-col gap-1 rounded-lg bg-muted/50 p-3">
          <p className="flex items-center gap-2 text-sm font-medium">
            Push on this device <Badge variant={push === "on" ? "default" : push === "off" ? "secondary" : "outline"}>{STATE_LABEL[push]}</Badge>
          </p>
          <p className="text-xs text-muted-foreground">{pushHint(push, browserOf(), isStandalone())}</p>
        </div>
      )}
      <SaveState save={save} />
    </Section>
  );
}
