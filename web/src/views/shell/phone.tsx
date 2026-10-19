// Touch pieces for the phone layout (UI design 4.2): a bottom sheet for
// actions, long-press, swipe on a card, pull to reconnect, and the
// offline banner. Hover cards and context menus have no touch equivalent.

import { CheckIcon, RefreshCwIcon, WifiOffIcon, XIcon } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Drawer, DrawerContent, DrawerDescription, DrawerFooter, DrawerHeader, DrawerTitle } from "@/components/ui/drawer";
import { cn } from "@/lib/utils";
import type { ConnStatus } from "../../store";

export interface SheetAction {
  id: string;
  label: string;
  destructive?: boolean;
  disabled?: boolean;
  run: () => void;
}

interface SheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;
  description?: ReactNode;
  /** Details above the actions. */
  children?: ReactNode;
  actions: SheetAction[];
}

/** A bottom sheet with full-width 44 px actions; an action closes it. */
export function ActionSheet({ open, onOpenChange, title, description, children, actions }: SheetProps) {
  return (
    <Drawer open={open} onOpenChange={onOpenChange}>
      <DrawerContent className="pb-[env(safe-area-inset-bottom)]">
        <DrawerHeader className="text-left">
          <DrawerTitle className="truncate">{title}</DrawerTitle>
          {description && <DrawerDescription>{description}</DrawerDescription>}
        </DrawerHeader>
        {children && <div className="min-h-0 overflow-y-auto px-4 pt-3">{children}</div>}
        <DrawerFooter className="pt-4">
          {actions.map((a) => (
            <Button
              key={a.id}
              type="button"
              variant={a.destructive ? "destructive" : "outline"}
              className="h-11 w-full"
              disabled={a.disabled}
              onClick={() => {
                onOpenChange(false);
                a.run();
              }}
            >
              {a.label}
            </Button>
          ))}
          <Button type="button" variant="ghost" className="h-11 w-full" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
        </DrawerFooter>
      </DrawerContent>
    </Drawer>
  );
}

/** How long a press holds before it counts as a long-press. */
export const LONG_PRESS_MS = 500;
/** A finger that moves this far scrolls instead of pressing. */
const SLOP = 10;

/**
 * Pointer handlers for a long-press. The click that ends a long-press is
 * swallowed, so the row does not also open.
 */
export function useLongPress(onLong: () => void, enabled = true) {
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const start = useRef<{ x: number; y: number } | null>(null);
  const fired = useRef(false);
  const cancel = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    start.current = null;
  };
  useEffect(() => cancel, []);
  if (!enabled) return {};
  return {
    onPointerDown: (e: React.PointerEvent) => {
      if (e.pointerType === "mouse") return;
      fired.current = false;
      start.current = { x: e.clientX, y: e.clientY };
      timer.current = setTimeout(() => {
        fired.current = true;
        cancel();
        navigator.vibrate?.(10);
        onLong();
      }, LONG_PRESS_MS);
    },
    onPointerMove: (e: React.PointerEvent) => {
      const s = start.current;
      if (s && Math.hypot(e.clientX - s.x, e.clientY - s.y) > SLOP) cancel();
    },
    onPointerUp: cancel,
    onPointerCancel: cancel,
    onContextMenu: (e: React.MouseEvent) => e.preventDefault(),
    onClickCapture: (e: React.MouseEvent) => {
      if (!fired.current) return;
      fired.current = false;
      e.preventDefault();
      e.stopPropagation();
    },
  };
}

/** The distance a card must travel to act: a third of its width, at most 96 px. */
export function swipeThreshold(width: number): number {
  return Math.min(96, Math.max(48, width / 3));
}

interface SwipeProps {
  /** Swipe right; `null` when the card has no such action. */
  right: { label: string; run: () => void } | null;
  left: { label: string; run: () => void; destructive?: boolean } | null;
  disabled?: boolean;
  className?: string;
  children: ReactNode;
}

/**
 * A card that moves with the finger. Past the threshold the label under it
 * turns solid; letting go there runs that side's action.
 */
export function Swipe({ right, left, disabled, className, children }: SwipeProps) {
  const [dx, setDx] = useState(0);
  const [dragging, setDragging] = useState(false);
  const self = useRef<HTMLDivElement>(null);
  const g = useRef<{ x: number; y: number; id: number; axis: "x" | "y" | null; moved: boolean } | null>(null);
  const width = self.current?.offsetWidth ?? 300;
  const limit = swipeThreshold(width);
  const past = Math.abs(dx) >= limit;
  const wasPast = useRef(false);
  if (past !== wasPast.current) {
    wasPast.current = past;
    if (past) navigator.vibrate?.(10);
  }

  const end = (e: React.PointerEvent) => {
    const s = g.current;
    if (!s || s.id !== e.pointerId) return;
    g.current = null;
    setDragging(false);
    const d = dx;
    setDx(0);
    if (s.axis !== "x") return;
    if (d >= limit && right) right.run();
    else if (d <= -limit && left) left.run();
  };

  const side = dx > 0 ? right : dx < 0 ? left : null;
  return (
    <div className={cn("relative overflow-hidden rounded-lg", className)}>
      {side && (
        <div
          aria-hidden
          data-armed={past || undefined}
          className={cn(
            "absolute inset-0 flex items-center px-5 text-sm font-medium transition-colors duration-100",
            dx > 0 ? "justify-start" : "justify-end",
            past
              ? dx > 0
                ? "bg-primary text-primary-foreground"
                : "bg-destructive text-white"
              : "bg-muted text-muted-foreground",
          )}
        >
          <span className={cn("flex items-center gap-1.5 transition-transform duration-100", past && "scale-110")}>
            {dx > 0 ? <CheckIcon className="size-4" /> : <XIcon className="size-4" />}
            {side.label}
          </span>
        </div>
      )}
      <div
        ref={self}
        data-swipe={dx === 0 ? undefined : dx > 0 ? "right" : "left"}
        className={cn("relative touch-pan-y bg-background", !dragging && "transition-transform duration-150 ease-out")}
        style={{ transform: dx ? `translateX(${dx}px)` : undefined }}
        onPointerDown={(e) => {
          if (disabled || e.pointerType === "mouse" || g.current) return;
          g.current = { x: e.clientX, y: e.clientY, id: e.pointerId, axis: null, moved: false };
        }}
        onPointerMove={(e) => {
          const s = g.current;
          if (!s || s.id !== e.pointerId) return;
          const x = e.clientX - s.x;
          const y = e.clientY - s.y;
          if (!s.axis) {
            if (Math.max(Math.abs(x), Math.abs(y)) < SLOP) return;
            s.axis = Math.abs(x) > Math.abs(y) ? "x" : "y";
            if (s.axis === "y") return;
            try {
              e.currentTarget.setPointerCapture(e.pointerId);
            } catch {
              // The pointer is gone already; the move events still arrive.
            }
            setDragging(true);
          }
          if (s.axis !== "x") return;
          s.moved = true;
          // No action on a side: the card resists.
          setDx((x > 0 && !right) || (x < 0 && !left) ? x / 4 : x);
        }}
        onPointerUp={end}
        onPointerCancel={(e) => {
          if (g.current?.id !== e.pointerId) return;
          g.current = null;
          setDragging(false);
          setDx(0);
        }}
        onClickCapture={(e) => {
          // A swipe is not a tap on the button under the finger.
          if (dragging || dx !== 0) {
            e.preventDefault();
            e.stopPropagation();
          }
        }}
      >
        {children}
      </div>
    </div>
  );
}

/** How far to pull before letting go reconnects. */
export const PULL_PX = 64;

/** Pull down at the top of `children` to run `onPull` (the needs list reconnects). */
export function PullToReconnect({ onPull, children }: { onPull: () => void; children: ReactNode }) {
  const [pull, setPull] = useState(0);
  const [busy, setBusy] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const pullRef = useRef(0);
  pullRef.current = pull;
  const onPullRef = useRef(onPull);
  onPullRef.current = onPull;

  useEffect(() => {
    const el = root.current;
    if (!el) return;
    let y0: number | null = null;
    const scroller = () => {
      for (let n: HTMLElement | null = el; n; n = n.parentElement) if (n.scrollTop > 0) return n;
      return null;
    };
    const down = (e: TouchEvent) => {
      y0 = e.touches.length === 1 && !scroller() ? e.touches[0].clientY : null;
    };
    const move = (e: TouchEvent) => {
      if (y0 === null) return;
      const dy = e.touches[0].clientY - y0;
      if (dy <= 0) return setPull(0);
      // Our own gesture: the page does not bounce.
      if (e.cancelable) e.preventDefault();
      setPull(Math.min(PULL_PX * 1.5, dy / 2));
    };
    const up = () => {
      if (y0 === null) return;
      y0 = null;
      if (pullRef.current >= PULL_PX) {
        setBusy(true);
        onPullRef.current();
        setTimeout(() => setBusy(false), 800);
      }
      setPull(0);
    };
    el.addEventListener("touchstart", down, { passive: true });
    el.addEventListener("touchmove", move, { passive: false });
    el.addEventListener("touchend", up);
    el.addEventListener("touchcancel", up);
    return () => {
      el.removeEventListener("touchstart", down);
      el.removeEventListener("touchmove", move);
      el.removeEventListener("touchend", up);
      el.removeEventListener("touchcancel", up);
    };
  }, []);

  const shown = busy ? PULL_PX / 2 : pull;
  return (
    <div ref={root} className="flex min-h-full flex-col">
      <div
        aria-hidden={!busy}
        role="status"
        className={cn(
          "flex shrink-0 items-end justify-center overflow-hidden text-xs text-muted-foreground",
          pull === 0 && "transition-[height] duration-150",
        )}
        style={{ height: shown }}
      >
        <span className="flex items-center gap-1.5 pb-1">
          <RefreshCwIcon
            className={cn("size-4", busy && "animate-spin")}
            style={busy ? undefined : { transform: `rotate(${(pull / PULL_PX) * 270}deg)` }}
          />
          {busy ? "Reconnecting…" : pull >= PULL_PX ? "Release to reconnect" : "Pull to reconnect"}
        </span>
      </div>
      {children}
    </div>
  );
}

/** Under the header while the link is down: what is wrong, and Retry. */
export function OfflineBanner({ status, seen, onRetry }: { status: ConnStatus; seen: boolean; onRetry: () => void }) {
  if (status === "open" || status === "unauthorized" || (status === "connecting" && !seen)) return null;
  const offline = status === "closed";
  return (
    <div
      role="alert"
      className={cn(
        "flex shrink-0 items-center gap-2 border-b py-1 pr-[max(0.5rem,env(safe-area-inset-right))] pl-[max(0.75rem,env(safe-area-inset-left))] text-sm",
        offline ? "bg-destructive/10 text-destructive" : "bg-muted text-muted-foreground",
      )}
    >
      {offline ? <WifiOffIcon className="size-4 shrink-0" /> : <RefreshCwIcon className="size-4 shrink-0 animate-spin" />}
      <span className="min-w-0 flex-1 truncate">{offline ? "Offline. Changes wait until you reconnect." : "Reconnecting…"}</span>
      <Button type="button" variant="outline" size="sm" className="h-11 min-w-11" onClick={onRetry}>
        Retry
      </Button>
    </div>
  );
}
