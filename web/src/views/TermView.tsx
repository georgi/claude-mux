import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import type { Conn } from "../conn";
import { useModel } from "../hooks";
import type { Store } from "../store";
import { Badge } from "@/components/ui/badge";

interface Props {
  term: number;
  title: string;
  conn: Conn;
  store: Store;
  /**
   * Fit the PTY to this screen (the default). Off, on a phone: the terminal
   * keeps the server's size and shrinks its font to fit instead.
   */
  fit?: boolean;
  /** Rewrites typed input first (the key bar's sticky Ctrl). */
  filter?: (data: string) => string;
}

const FONT = 13;

/** Resolve a CSS color (any syntax, e.g. oklch) to rgb through a canvas. */
function resolveColor(el: HTMLElement, prop: string): string {
  const probe = document.createElement("span");
  probe.style.color = `var(${prop})`;
  probe.style.display = "none";
  el.appendChild(probe);
  const c = getComputedStyle(probe).color;
  probe.remove();
  return c;
}

/** The xterm theme from the CSS theme tokens. */
function themeFrom(el: HTMLElement) {
  return {
    background: resolveColor(el, "--background"),
    foreground: resolveColor(el, "--foreground"),
    cursor: resolveColor(el, "--foreground"),
    cursorAccent: resolveColor(el, "--background"),
    selectionBackground: resolveColor(el, "--accent"),
  };
}

/**
 * One terminal in xterm.js. Size ownership (architecture 6.2): the PTY
 * follows one client. Focusing or typing here makes this browser the owner at
 * its fitted size; while another client owns it, xterm takes that client's
 * size so the screen lines up.
 */
export function TermView({ term, title, conn, store, fit: fitPty = true, filter }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const m = useModel(store);
  const filterRef = useRef(filter);
  filterRef.current = filter;

  useEffect(() => {
    const el = host.current;
    if (!el) return;
    const xt = new Terminal({
      cursorBlink: true,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      fontSize: FONT,
      scrollback: 5000,
      theme: themeFrom(el),
    });
    const fit = new FitAddon();
    xt.loadAddon(fit);
    xt.open(el);
    // Under automation the DOM renderer stays, so tests can read the screen.
    if (!navigator.webdriver) {
      try {
        const gl = new WebglAddon();
        gl.onContextLoss(() => gl.dispose());
        xt.loadAddon(gl);
      } catch {
        // No WebGL: xterm's DOM renderer stays.
      }
    }

    const isOwner = () => {
      const s = store.get();
      return s.clientId !== null && s.sizes[term]?.owner === s.clientId;
    };
    const fitted = () => {
      const d = fit.proposeDimensions();
      return d && d.cols > 0 && d.rows > 0 ? { rows: d.rows, cols: d.cols } : null;
    };
    const takeSize = () => {
      const d = fitted();
      if (!d) return;
      if (d.cols !== xt.cols || d.rows !== xt.rows) xt.resize(d.cols, d.rows);
      conn.focus(term, d.rows, d.cols);
    };
    /** The font size at which `cols` columns fill the host's width. */
    const shrinkFont = (cols: number) => {
      const d = fit.proposeDimensions();
      if (!d || d.cols <= 0) return;
      const size = Math.max(5, Math.min(FONT, Math.floor((xt.options.fontSize ?? FONT) * (d.cols / cols) * 10) / 10));
      if (size !== xt.options.fontSize) xt.options.fontSize = size;
    };
    const followOwner = () => {
      const s = store.get().sizes[term];
      if (!s || (fitPty && isOwner())) return;
      if (s.cols !== xt.cols || s.rows !== xt.rows) xt.resize(s.cols, s.rows);
      if (!fitPty) shrinkFont(s.cols);
    };

    const applyTheme = () => {
      xt.options.theme = themeFrom(el);
    };
    const mo = new MutationObserver(applyTheme);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["class", "data-theme"] });
    const mq = typeof matchMedia === "function" ? matchMedia("(prefers-color-scheme: dark)") : null;
    mq?.addEventListener("change", applyTheme);

    conn.want(term, { reset: () => xt.reset(), write: (d) => xt.write(d) });
    const offStore = store.subscribe(followOwner);
    const data = xt.onData((d) => {
      if (fitPty && !isOwner()) takeSize();
      conn.input(term, filterRef.current ? filterRef.current(d) : d);
    });
    const bin = xt.onBinary((d) => conn.input(term, Uint8Array.from(d, (c) => c.charCodeAt(0) & 0xff)));
    const onFocus = () => fitPty && takeSize();
    if (fitPty) {
      xt.options.fontSize = FONT;
    } else {
      followOwner();
    }
    xt.textarea?.addEventListener("focus", onFocus);
    const ro = new ResizeObserver(() => {
      if (!fitPty) return followOwner();
      if (!isOwner()) return;
      const d = fitted();
      if (!d || (d.cols === xt.cols && d.rows === xt.rows)) return;
      xt.resize(d.cols, d.rows);
      conn.resize(term, d.rows, d.cols);
    });
    ro.observe(el);
    // Focus takes the size; on a phone it would also pop up the keyboard.
    if (fitPty) xt.focus();

    return () => {
      ro.disconnect();
      mo.disconnect();
      mq?.removeEventListener("change", applyTheme);
      offStore();
      data.dispose();
      bin.dispose();
      xt.textarea?.removeEventListener("focus", onFocus);
      conn.release(term);
      xt.dispose();
    };
  }, [term, conn, store, fitPty]);

  const size = m.sizes[term];
  const other = size && size.owner !== null && size.owner !== m.clientId;
  const exit = m.exits[term];
  return (
    <section className="flex h-full min-h-0 w-full min-w-0 flex-col overflow-hidden rounded-lg border bg-background">
      <header className="flex h-8 shrink-0 items-center gap-2 border-b bg-card px-3 text-xs">
        <span className="min-w-0 flex-1 truncate font-medium">{title}</span>
        {size && (
          <span className="shrink-0 font-mono text-muted-foreground">
            {size.cols}×{size.rows}
          </span>
        )}
        {other && fitPty && <Badge variant="outline">sized by another client; click to take over</Badge>}
        {exit && <Badge variant="destructive">exited {exit.signal ?? exit.code}</Badge>}
      </header>
      <div className="xterm-host min-h-0 min-w-0 flex-1 overflow-hidden p-1" ref={host} />
    </section>
  );
}
