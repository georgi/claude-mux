// Motion and focus: View Transitions on route changes, focus on the page
// heading after one, and the enter animation of new list rows.

import { useCallback, useEffect, useRef } from "react";
import { flushSync } from "react-dom";

type WithTransition = Document & { startViewTransition?: (update: () => void) => unknown };

export function reducedMotion(): boolean {
  return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** Runs `update` inside `document.startViewTransition`, else runs it at once. */
export function viewTransition(update: () => void): void {
  const d = document as WithTransition;
  if (typeof d.startViewTransition !== "function" || reducedMotion() || document.visibilityState !== "visible") {
    update();
    return;
  }
  try {
    d.startViewTransition(() => flushSync(update));
  } catch {
    update();
  }
}

/** The id of the page heading in the header (shell/header.tsx). */
export const PAGE_HEADING = "page-heading";

/**
 * Moves focus to the page heading, unless focus is in the tree: there the
 * arrow keys keep moving through the rows after a row opens.
 */
export function focusHeading(): void {
  const active = document.activeElement;
  if (active?.closest('[role="tree"]')) return;
  const h = document.getElementById(PAGE_HEADING);
  h?.focus({ preventScroll: true });
}

/** Fades and slides a new list row in, with the motion tokens from index.css. */
export function animateEnter(el: Element): void {
  if (reducedMotion() || typeof el.animate !== "function") return;
  const css = getComputedStyle(document.documentElement);
  const ms = Number.parseFloat(css.getPropertyValue("--duration-base")) || 0;
  if (!ms) return;
  el.animate([{ opacity: 0, transform: "translateY(-0.25rem)" }, { opacity: 1, transform: "none" }], {
    duration: ms,
    easing: css.getPropertyValue("--ease-out").trim() || "ease-out",
  });
}

/**
 * A ref helper for list rows: a row that joins after the first render
 * animates in. Rows of the first render, and rows seen before, do not.
 */
export function useEnter(): (key: string | number, el: Element | null) => void {
  const known = useRef(new Set<string | number>());
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
  }, []);
  return useCallback((key, el) => {
    if (!el || known.current.has(key)) return;
    known.current.add(key);
    if (mounted.current) animateEnter(el);
  }, []);
}
