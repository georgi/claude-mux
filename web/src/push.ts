// Web Push on this device: the stored kind choices, the subscription and
// the service worker it hangs on.

import type { Api } from "./api/http";
import type { NotifyKind } from "./api/types";

/** The item kinds a device can choose to be notified about. */
export const NOTIFY_KINDS = [
  ["permission", "Permission requests"],
  ["question", "Questions"],
  ["plan_review", "Plans ready for review"],
  ["stalled", "Stalled or exited agents"],
  ["ci_failing", "CI failing"],
  ["plan_done", "Finished plans"],
] as const satisfies readonly (readonly [NotifyKind, string])[];

const NOTIFY_KEY = "cmux.notify";

export function loadNotify(): Record<string, boolean> {
  try {
    const raw = localStorage.getItem(NOTIFY_KEY);
    if (raw) return JSON.parse(raw) as Record<string, boolean>;
  } catch {
    // Private mode or bad JSON: the defaults.
  }
  return Object.fromEntries(NOTIFY_KINDS.map(([k]) => [k, true]));
}

export function saveNotify(on: Record<string, boolean>): void {
  try {
    localStorage.setItem(NOTIFY_KEY, JSON.stringify(on));
  } catch {
    // Not stored; it still applies until reload.
  }
}

/** The kinds that are on; a kind missing from the choices is on. */
export function kindsOf(on: Record<string, boolean>): NotifyKind[] {
  return NOTIFY_KINDS.map(([k]) => k).filter((k) => on[k] !== false);
}

/** URL-safe base64, padded or not, to bytes. */
export function b64url(s: string): Uint8Array<ArrayBuffer> {
  const b64 = s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "=");
  return Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
}

export type PushState = "unsupported" | "insecure" | "denied" | "off" | "on";

/** Register the service worker (production builds only). */
export function registerSw(): Promise<ServiceWorkerRegistration> {
  return navigator.serviceWorker.register("/sw.js", { scope: "/" });
}

async function current(): Promise<PushSubscription | null> {
  const reg = await navigator.serviceWorker.getRegistration("/");
  return (await reg?.pushManager.getSubscription()) ?? null;
}

export async function pushState(): Promise<PushState> {
  if (!window.isSecureContext) return "insecure";
  if (!("serviceWorker" in navigator) || !("PushManager" in window) || !("Notification" in window)) return "unsupported";
  if (Notification.permission === "denied") return "denied";
  return Notification.permission === "granted" && (await current()) ? "on" : "off";
}

async function send(api: Api, sub: PushSubscription, kinds: NotifyKind[]): Promise<void> {
  const keys = sub.toJSON().keys ?? {};
  await api.pushSubscribe({ endpoint: sub.endpoint, keys: { p256dh: keys.p256dh ?? "", auth: keys.auth ?? "" }, kinds });
}

export async function enablePush(api: Api, kinds: NotifyKind[]): Promise<PushState> {
  if ((await Notification.requestPermission()) !== "granted") return pushState();
  await registerSw();
  const reg = await navigator.serviceWorker.ready;
  let sub = await reg.pushManager.getSubscription();
  if (!sub) {
    const { public_key } = await api.vapid();
    sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64url(public_key) });
  }
  await send(api, sub, kinds);
  return "on";
}

/** Tell the server the new kinds, when this device is subscribed. */
export async function updateKinds(api: Api, kinds: NotifyKind[]): Promise<void> {
  const sub = await current();
  if (sub) await send(api, sub, kinds);
}

export async function disablePush(api: Api): Promise<void> {
  const sub = await current();
  if (!sub) return;
  await sub.unsubscribe();
  await api.pushUnsubscribe(sub.endpoint);
}
