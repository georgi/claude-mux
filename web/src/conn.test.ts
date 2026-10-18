import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ClientMsg, ServerMsg } from "./api/types";
import { Conn, type SocketLike, streamUrl } from "./conn";
import { entry, state, task } from "./fixtures";
import { Store } from "./store";

class FakeSocket implements SocketLike {
  binaryType = "blob";
  readyState = 0;
  sent: (string | Uint8Array)[] = [];
  onopen: ((ev: unknown) => void) | null = null;
  onclose: ((ev: unknown) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  send(d: string | ArrayBufferLike | ArrayBufferView) {
    this.sent.push(typeof d === "string" ? d : new Uint8Array(d as ArrayBuffer));
  }
  close() {
    if (this.readyState === 3) return;
    this.readyState = 3;
    this.onclose?.({});
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  recv(m: ServerMsg) {
    this.onmessage?.({ data: JSON.stringify(m) });
  }
  recvBinary(bytes: number[]) {
    this.onmessage?.({ data: new Uint8Array(bytes).buffer });
  }
  msgs(): ClientMsg[] {
    return this.sent.filter((s): s is string => typeof s === "string").map((s) => JSON.parse(s));
  }
}

const welcome: ServerMsg = {
  type: "welcome",
  rev: 4,
  server: { version: "0.1.0", name: "vps" },
  client_id: "c-1",
  scope: "full",
};

/** An output frame for term 1. */
function output(seq: number, text: string, kind = 2): number[] {
  const head = [kind, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, seq];
  return [...head, ...new TextEncoder().encode(text)];
}

describe("Conn", () => {
  let sockets: FakeSocket[];
  let store: Store;
  let conn: Conn;
  const sink = { out: "", resets: 0, reset() { this.resets++; this.out = ""; }, write(d: Uint8Array) { this.out += new TextDecoder().decode(d); } };

  beforeEach(() => {
    vi.useFakeTimers();
    sockets = [];
    store = new Store();
    sink.out = "";
    sink.resets = 0;
    conn = new Conn(store, {
      url: "ws://x/v1/stream",
      socket: () => {
        const s = new FakeSocket();
        sockets.push(s);
        return s;
      },
      pingEvery: 1000,
    });
  });
  afterEach(() => {
    conn.stop();
    vi.useRealTimers();
  });

  it("builds the stream url from the page location", () => {
    expect(streamUrl({ protocol: "https:", host: "h:1" })).toBe("wss://h:1/v1/stream");
    expect(streamUrl({ protocol: "http:", host: "h" })).toBe("ws://h/v1/stream");
  });

  it("says hello, subscribes, and feeds frames by seq", () => {
    conn.start();
    const s = sockets[0];
    s.open();
    expect(s.binaryType).toBe("arraybuffer");
    expect(s.msgs()[0]).toMatchObject({ type: "hello", client: "web", since_rev: null, token: null, terms: [] });
    s.recv(welcome);
    s.recv({ type: "state", rev: 4, snapshot: state({ rev: 4 }) });
    expect(store.get().conn).toBe("open");
    conn.want(1, sink);
    expect(s.msgs().at(-1)).toEqual({ type: "subscribe", term: 1, since_seq: null });
    s.recvBinary(output(10, "AB", 3));
    expect(sink.resets).toBe(1);
    expect(conn.nextSeq(1)).toBe(10);
    s.recvBinary(output(10, "cd"));
    // Overlap: bytes 10..12 again, then 12..13 new.
    s.recvBinary(output(10, "cde"));
    expect(sink.out).toBe("ABcde");
    expect(conn.nextSeq(1)).toBe(13);
    conn.input(1, "x");
    expect([...(s.sent.at(-1) as Uint8Array)]).toEqual([1, 0, 0, 0, 1, 0x78]);
  });

  it("reconnects with the last rev and seq, and restores focus and size", () => {
    conn.start();
    let s = sockets[0];
    s.open();
    s.recv(welcome);
    s.recv({ type: "state", rev: 4, snapshot: state({ rev: 4 }) });
    conn.want(1, sink);
    s.recvBinary(output(0, "hi", 3));
    s.recvBinary(output(0, "abc"));
    conn.focus(1, 30, 100);
    s.recv({ type: "deltas", rev: 7, items: [{ rev: 7, change: { kind: "task", value: task("T-0001") } }] });
    s.close();
    expect(store.get().conn).toBe("closed");
    vi.advanceTimersByTime(1000);
    expect(sockets).toHaveLength(2);
    s = sockets[1];
    s.open();
    expect(s.msgs()[0]).toMatchObject({ type: "hello", since_rev: 7, terms: [{ id: 1, since_seq: 3 }] });
    s.recv({ ...welcome, rev: 7 });
    expect(s.msgs().slice(1)).toEqual([
      { type: "focus", term: 1 },
      { type: "resize", term: 1, rows: 30, cols: 100 },
    ]);
    // The missed bytes resume the terminal without a reset.
    s.recvBinary(output(3, "def"));
    expect(sink.out).toBe("hiabcdef");
    expect(sink.resets).toBe(1);
  });

  it("streams a chat into the store and resumes it after the last entry", () => {
    const a = "P-20261018-demo/T-0002";
    conn.start();
    let s = sockets[0];
    s.open();
    s.recv(welcome);
    conn.subChat(a);
    conn.subChat(a);
    expect(s.msgs().filter((m) => m.type === "sub_chat")).toEqual([{ type: "sub_chat", agent: a, after: null }]);
    s.recv({ type: "chat", agent: a, entries: [entry("1"), entry("2")] });
    s.recv({ type: "chat", agent: a, entries: [entry("2"), entry("3")] });
    expect(store.get().chats[a].map((e) => e.uuid)).toEqual(["1", "2", "3"]);
    s.close();
    vi.advanceTimersByTime(1000);
    s = sockets[1];
    s.open();
    s.recv(welcome);
    expect(s.msgs().slice(1)).toEqual([{ type: "sub_chat", agent: a, after: "3" }]);
    // Two views held it; the last one to let go unsubscribes.
    conn.unsubChat(a);
    expect(s.msgs().some((m) => m.type === "unsub_chat")).toBe(false);
    conn.unsubChat(a);
    expect(s.msgs().at(-1)).toEqual({ type: "unsub_chat", agent: a });
  });

  it("subscribes after welcome to terminals that never got a byte", () => {
    conn.want(1, sink);
    conn.start();
    const s = sockets[0];
    s.open();
    expect(s.msgs()[0]).toMatchObject({ type: "hello", terms: [] });
    s.recv(welcome);
    expect(s.msgs()[1]).toEqual({ type: "subscribe", term: 1, since_seq: null });
  });

  it("drops a socket that stops answering pings", () => {
    conn.start();
    const s = sockets[0];
    s.open();
    s.recv(welcome);
    vi.advanceTimersByTime(1000);
    expect(s.msgs().at(-1)).toMatchObject({ type: "ping" });
    vi.advanceTimersByTime(1000);
    expect(s.readyState).toBe(3);
    expect(store.get().conn).toBe("closed");
    vi.advanceTimersByTime(1000);
    expect(sockets).toHaveLength(2);
  });

  it("backs off between failed attempts and stops when unauthorized", () => {
    conn.start();
    sockets[0].close();
    vi.advanceTimersByTime(700);
    expect(sockets).toHaveLength(2);
    sockets[1].close();
    vi.advanceTimersByTime(700);
    expect(sockets).toHaveLength(2);
    vi.advanceTimersByTime(700);
    expect(sockets).toHaveLength(3);
    const s = sockets[2];
    s.open();
    s.recv({ type: "error", code: "unauthorized", message: "no session" });
    s.close();
    vi.advanceTimersByTime(60_000);
    expect(sockets).toHaveLength(3);
    expect(store.get().conn).toBe("unauthorized");
  });
});
