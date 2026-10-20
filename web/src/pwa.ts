// Installing the app: Chrome's `beforeinstallprompt`, kept from page load
// until the user taps "Install app", and the iOS Safari hint.

import { useEffect, useState } from "react";

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

// The on-screen keyboard. iOS keeps the layout viewport when the keyboard
// opens and only shrinks the visual one, so a bottom bar would sit under
// the keyboard. The phone layout takes its size from the visual viewport.

export interface Viewport {
  /** The visible height in CSS px. */
  height: number;
  /** How far the browser scrolled the visual viewport down. */
  top: number;
  /** The height the keyboard (or other UI) covers at the bottom. */
  keyboard: number;
}

/** Below this, the covered height is browser chrome, not a keyboard. */
export const KEYBOARD_PX = 120;

export function readViewport(w: Pick<Window, "innerHeight" | "visualViewport"> = window): Viewport {
  const vv = w.visualViewport;
  if (!vv) return { height: w.innerHeight, top: 0, keyboard: 0 };
  return { height: vv.height, top: vv.offsetTop, keyboard: Math.max(0, Math.round(w.innerHeight - vv.height - vv.offsetTop)) };
}

/**
 * Scroll logs that sit at their bottom (`role="log"`) back to it after a
 * resize, so the newest message stays above the composer. Call before the
 * resize lands in the layout.
 */
export function keepLogsAtBottom(root: ParentNode = document): () => void {
  const logs = [...root.querySelectorAll<HTMLElement>('[role="log"]')].filter(
    (el) => el.scrollHeight - el.scrollTop - el.clientHeight < 8,
  );
  return () => {
    for (const el of logs) el.scrollTop = el.scrollHeight;
  };
}

/** The visual viewport while `on`; `null` otherwise. */
export function useVisualViewport(on: boolean): Viewport | null {
  const [vp, setVp] = useState<Viewport | null>(() => (on ? readViewport() : null));
  useEffect(() => {
    if (!on) return setVp(null);
    const vv = window.visualViewport;
    const update = () => {
      const restore = keepLogsAtBottom();
      setVp((old) => {
        const next = readViewport();
        return old && old.height === next.height && old.top === next.top && old.keyboard === next.keyboard ? old : next;
      });
      requestAnimationFrame(restore);
    };
    update();
    vv?.addEventListener("resize", update);
    vv?.addEventListener("scroll", update);
    window.addEventListener("resize", update);
    return () => {
      vv?.removeEventListener("resize", update);
      vv?.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
    };
  }, [on]);
  return vp;
}
