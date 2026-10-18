// Installing the app: Chrome's `beforeinstallprompt`, kept from page load
// until the user taps "Install app", and the iOS Safari hint.

import { useEffect, useState } from "preact/hooks";

interface InstallPrompt extends Event {
  prompt(): Promise<void>;
  userChoice: Promise<{ outcome: "accepted" | "dismissed" }>;
}

let deferred: InstallPrompt | null = null;
const listeners = new Set<() => void>();
const changed = () => listeners.forEach((f) => f());

/** Call once at start, before the browser fires the event. */
export function captureInstall(): void {
  window.addEventListener("beforeinstallprompt", (e) => {
    e.preventDefault();
    deferred = e as InstallPrompt;
    changed();
  });
  window.addEventListener("appinstalled", () => {
    deferred = null;
    changed();
  });
}

export function isStandalone(): boolean {
  return window.matchMedia?.("(display-mode: standalone)").matches || (navigator as { standalone?: boolean }).standalone === true;
}

/** iOS has no install prompt; iPadOS reports itself as a Mac with touch. */
export function isIos(ua = navigator.userAgent, touch = navigator.maxTouchPoints): boolean {
  return /iPhone|iPad|iPod/.test(ua) || (/Macintosh/.test(ua) && touch > 1);
}

/** The install prompt, when the browser offers one now. */
export function useInstall(): (() => Promise<void>) | null {
  const [, setN] = useState(0);
  useEffect(() => {
    const f = () => setN((n) => n + 1);
    listeners.add(f);
    return () => void listeners.delete(f);
  }, []);
  if (!deferred) return null;
  const p = deferred;
  return async () => {
    await p.prompt();
    await p.userChoice;
    deferred = null;
    changed();
  };
}
