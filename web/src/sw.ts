// The service worker, built to `dist/sw.js` (scope `/`). It keeps the app
// shell for an offline start, shows Web Push notifications and answers
// permission requests and questions from their buttons. /v1/ is never cached.

import { answerFor, clickUrl, notification, parsePayload } from "./notify";
import type { PushPayload } from "./api/types";

// The DOM lib types this file; the worker types it needs are declared here.
interface ExtEvent extends Event {
  waitUntil(p: Promise<unknown>): void;
}
interface FetchEv extends ExtEvent {
  request: Request;
  respondWith(r: Promise<Response>): void;
}
interface PushEv extends ExtEvent {
  data: { text(): string } | null;
}
interface ClickEv extends ExtEvent {
  action: string;
  reply?: string | null;
  notification: Notification;
}
interface WinClient {
  url: string;
  focus(): Promise<WinClient>;
  navigate(url: string): Promise<WinClient | null>;
}
interface Scope {
  registration: ServiceWorkerRegistration;
  clients: {
    claim(): Promise<void>;
    matchAll(o: { type: "window"; includeUncontrolled: boolean }): Promise<WinClient[]>;
    openWindow(url: string): Promise<WinClient | null>;
  };
  skipWaiting(): Promise<void>;
  addEventListener(type: string, f: (e: never) => void): void;
}
const sw = self as unknown as Scope;

const CACHE = "cmux-shell-v1";
const SHELL = ["/", "/manifest.webmanifest", "/icon.svg", "/icon-192.png", "/icon-512.png", "/apple-touch-icon.png"];

/** Cache index.html and the hashed assets it names; drop assets it no longer names. */
async function keepShell(html: Response): Promise<void> {
  const cache = await caches.open(CACHE);
  const text = await html.clone().text();
  const assets = new Set([...text.matchAll(/["'](\/assets\/[^"']+)["']/g)].map((m) => m[1]));
  await cache.put("/", html);
  for (const req of await cache.keys()) {
    const path = new URL(req.url).pathname;
    if (path.startsWith("/assets/") && !assets.has(path)) await cache.delete(req);
  }
  await Promise.all(
    [...assets].map(async (a) => {
      if (!(await cache.match(a))) await cache.add(a).catch(() => {});
    }),
  );
}

sw.addEventListener("install", (e: ExtEvent) => {
  e.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE);
      await cache.addAll(SHELL.slice(1)).catch(() => {});
      const r = await fetch("/", { cache: "no-cache" });
      if (r.ok) await keepShell(r);
      await sw.skipWaiting();
    })(),
  );
});

sw.addEventListener("activate", (e: ExtEvent) => {
  e.waitUntil(
    (async () => {
      for (const k of await caches.keys()) if (k !== CACHE) await caches.delete(k);
      await sw.clients.claim();
    })(),
  );
});

sw.addEventListener("fetch", (e: FetchEv) => {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== "GET" || url.origin !== location.origin || url.pathname.startsWith("/v1/")) return;
  if (req.mode === "navigate") {
    // Network first: the newest shell when online, the cached one when not.
    e.respondWith(
      fetch(req)
        .then((r) => {
          if (r.ok) e.waitUntil(keepShell(r.clone()));
          return r;
        })
        .catch(async () => (await caches.match("/")) ?? Response.error()),
    );
  } else if (url.pathname.startsWith("/assets/")) {
    // Hashed names never change: the cache first.
    e.respondWith(
      caches.match(req).then(
        (hit) =>
          hit ??
          fetch(req).then((r) => {
            if (r.ok) {
              const copy = r.clone();
              e.waitUntil(caches.open(CACHE).then((c) => c.put(req, copy)));
            }
            return r;
          }),
      ),
    );
  } else if (SHELL.includes(url.pathname)) {
    e.respondWith(fetch(req).catch(async () => (await caches.match(req)) ?? Response.error()));
  }
});

sw.addEventListener("push", (e: PushEv) => {
  const p = e.data ? parsePayload(e.data.text()) : null;
  if (!p) return;
  const n = notification(p);
  e.waitUntil(sw.registration.showNotification(n.title, n.options as NotificationOptions));
});

async function openApp(url: string): Promise<void> {
  const wins = await sw.clients.matchAll({ type: "window", includeUncontrolled: true });
  const win = wins.find((w) => new URL(w.url).origin === location.origin);
  if (!win) {
    await sw.clients.openWindow(url);
    return;
  }
  await win.focus();
  await win.navigate(url).catch(() => null);
}

async function onClick(p: PushPayload, action: string, reply: string | null): Promise<void> {
  const req = answerFor(p, action, reply);
  if (req && p.item !== null) {
    try {
      const r = await fetch(`/v1/items/${p.item}/answer`, {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json", "X-Cmux": "1", "Idempotency-Key": crypto.randomUUID() },
        body: JSON.stringify(req),
      });
      if (r.ok) return;
    } catch {
      // Offline or the session is gone: the app shows the item.
    }
  }
  await openApp(clickUrl(p));
}

sw.addEventListener("notificationclick", (e: ClickEv) => {
  e.notification.close();
  const p = e.notification.data as PushPayload | null;
  if (!p) return e.waitUntil(openApp("/"));
  e.waitUntil(onClick(p, e.action, e.reply ?? null));
});
