// The stream connection (architecture 6.2): one WebSocket, `hello` with the
// last `rev` and each wanted terminal's next `seq`, then live messages into
// the store and terminal bytes into the open terminals' sinks. It reconnects
// with backoff after a close, a missed pong, or the browser coming online.
//
// The session cookie authenticates the upgrade, so `hello` carries no token.

import { decodeFrame, encodeInput } from "./api/frames";
import type { ClientMsg, ServerMsg, TermId, TermPos } from "./api/types";
import type { Store } from "./store";

/** Where a terminal's bytes go (an xterm.js instance). */
export interface TermSink {
  /** A snapshot follows: reset the parser. */
  reset(): void;
  write(data: Uint8Array): void;
}

export interface SocketLike {
  binaryType: string;
  readyState: number;
  send(data: string | ArrayBufferLike | ArrayBufferView): void;
  close(code?: number, reason?: string): void;
  onopen: ((ev: unknown) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
}

export interface ConnOptions {
  url: string;
  socket?: (url: string) => SocketLike;
  /** Milliseconds between pings; a ping without any answer by the next one closes the socket. */
  pingEvery?: number;
  info?: string;
}

const OPEN = 1;
const MIN_BACKOFF = 500;
const MAX_BACKOFF = 10_000;

export function streamUrl(loc: { protocol: string; host: string }): string {
  return `${loc.protocol === "https:" ? "wss" : "ws"}://${loc.host}/v1/stream`;
}

export class Conn {
  private ws: SocketLike | null = null;
  private stopped = false;
  private backoff = MIN_BACKOFF;
  private retry: ReturnType<typeof setTimeout> | null = null;
  private pinger: ReturnType<typeof setInterval> | null = null;
  /** Something arrived since the last ping. */
  private heard = true;
  /** Wanted terminals and the offset after the last byte applied (`null` before any). */
  private terms = new Map<TermId, number | null>();
  private sinks = new Map<TermId, TermSink>();
  private focused: TermId | null = null;
  private sizes = new Map<TermId, { rows: number; cols: number }>();

  constructor(
    private store: Store,
    private opts: ConnOptions,
  ) {}

  start(): void {
    this.stopped = false;
    this.connect();
  }

  stop(): void {
    this.stopped = true;
    this.clearTimers();
    this.ws?.close(1000);
    this.ws = null;
  }

  /** Reconnect now (the browser came back online, or the user asked). */
  kick(): void {
    if (this.stopped) return;
    this.backoff = MIN_BACKOFF;
    if (this.ws && this.ws.readyState === OPEN) {
      // A socket that survived a network change may be dead; a ping proves it.
      this.send({ type: "ping", nonce: Date.now() });
      return;
    }
    this.ws?.close();
    this.ws = null;
    this.connect();
  }

  /** The offset after the last byte applied to `term`. */
  nextSeq(term: TermId): number | null {
    return this.terms.get(term) ?? null;
  }

  hello(): ClientMsg {
    const terms: TermPos[] = [];
    for (const [id, next] of this.terms) if (next !== null) terms.push({ id, since_seq: next });
    return {
      type: "hello",
      client: "web",
      since_rev: this.store.get().view?.rev ?? null,
      token: null,
      info: this.opts.info ?? null,
      terms,
    };
  }

  /** What follows `welcome`: terminals the hello could not name, then focus and size. */
  afterWelcome(): ClientMsg[] {
    const out: ClientMsg[] = [];
    for (const [id, next] of this.terms) if (next === null) out.push({ type: "subscribe", term: id, since_seq: null });
    const f = this.focused;
    if (f !== null && this.terms.has(f)) {
      out.push({ type: "focus", term: f });
      const s = this.sizes.get(f);
      if (s) out.push({ type: "resize", term: f, rows: s.rows, cols: s.cols });
    }
    return out;
  }

  /** Stream `term` into `sink`. */
  want(term: TermId, sink: TermSink): void {
    this.sinks.set(term, sink);
    // A new sink starts empty, so it needs a snapshot, not the missed bytes.
    this.terms.set(term, null);
    this.send({ type: "subscribe", term, since_seq: null });
  }

  release(term: TermId): void {
    this.terms.delete(term);
    this.sinks.delete(term);
    this.sizes.delete(term);
    if (this.focused === term) this.focused = null;
    this.send({ type: "unsubscribe", term });
  }

  /** Take the size ownership of `term` at this size. */
  focus(term: TermId, rows: number, cols: number): void {
    this.focused = term;
    this.sizes.set(term, { rows, cols });
    this.send({ type: "focus", term });
    this.send({ type: "resize", term, rows, cols });
  }

  /** Report a new viewport size (only the owner should). */
  resize(term: TermId, rows: number, cols: number): void {
    this.sizes.set(term, { rows, cols });
    this.send({ type: "resize", term, rows, cols });
  }

  input(term: TermId, data: string | Uint8Array): void {
    if (this.ws?.readyState === OPEN) this.ws.send(encodeInput(term, data));
  }

  private send(m: ClientMsg): void {
    if (this.ws?.readyState === OPEN) this.ws.send(JSON.stringify(m));
  }

  private connect(): void {
    this.clearTimers();
    this.store.dispatch({ type: "conn", status: "connecting" });
    const make = this.opts.socket ?? ((u: string) => new WebSocket(u) as unknown as SocketLike);
    const ws = make(this.opts.url);
    ws.binaryType = "arraybuffer";
    this.ws = ws;
    ws.onopen = () => {
      if (this.ws !== ws) return;
      ws.send(JSON.stringify(this.hello()));
      this.heard = true;
      const every = this.opts.pingEvery ?? 15_000;
      this.pinger = setInterval(() => this.ping(ws), every);
    };
    ws.onmessage = (ev) => {
      if (this.ws !== ws) return;
      this.heard = true;
      if (typeof ev.data === "string") this.onText(ev.data);
      else if (ev.data instanceof ArrayBuffer) this.onBinary(ev.data);
    };
    ws.onerror = () => {};
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      this.clearTimers();
      const unauthorized = this.store.get().conn === "unauthorized";
      if (!unauthorized) this.store.dispatch({ type: "conn", status: "closed" });
      if (this.stopped || unauthorized) return;
      const wait = this.backoff * (0.75 + Math.random() * 0.5);
      this.backoff = Math.min(this.backoff * 2, MAX_BACKOFF);
      this.retry = setTimeout(() => this.connect(), wait);
    };
  }

  private ping(ws: SocketLike): void {
    if (!this.heard) {
      // Nothing since the last ping: the network dropped under us.
      ws.close();
      ws.onclose?.({});
      return;
    }
    this.heard = false;
    if (ws.readyState === OPEN) ws.send(JSON.stringify({ type: "ping", nonce: Date.now() } satisfies ClientMsg));
  }

  private clearTimers(): void {
    if (this.retry) clearTimeout(this.retry);
    if (this.pinger) clearInterval(this.pinger);
    this.retry = null;
    this.pinger = null;
  }

  private onText(text: string): void {
    let m: ServerMsg;
    try {
      m = JSON.parse(text) as ServerMsg;
    } catch {
      return;
    }
    if (m.type === "welcome") {
      this.backoff = MIN_BACKOFF;
      this.store.dispatch(m);
      for (const x of this.afterWelcome()) this.send(x);
      return;
    }
    this.store.dispatch(m);
  }

  private onBinary(buf: ArrayBuffer): void {
    const f = decodeFrame(buf);
    if (!f || !this.terms.has(f.term)) return;
    const sink = this.sinks.get(f.term);
    if (f.kind === "snapshot") {
      sink?.reset();
      if (f.data.length) sink?.write(f.data);
      this.terms.set(f.term, f.seq);
      return;
    }
    const next = this.terms.get(f.term) ?? null;
    let data = f.data;
    if (next !== null && f.seq < next) {
      // Bytes we already have (a resubscribe overlaps): skip them.
      const skip = next - f.seq;
      if (skip >= data.length) return;
      data = data.subarray(skip);
    }
    sink?.write(data);
    this.terms.set(f.term, f.seq + f.data.length);
  }
}
