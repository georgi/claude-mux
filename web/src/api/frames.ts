// Binary terminal frames (cmux_proto::stream::Frame).
//
// `[kind u8][term u32 BE]` for input, plus `[seq u64 BE]` for output and
// snapshot frames; the payload follows. Sequence numbers are byte offsets and
// stay far below 2^53, so they are plain numbers here.

export const KIND_INPUT = 0x01;
export const KIND_OUTPUT = 0x02;
export const KIND_SNAPSHOT = 0x03;

export const INPUT_HEADER = 1 + 4;
export const OUTPUT_HEADER = 1 + 4 + 8;

export type Frame =
  | { kind: "output"; term: number; seq: number; data: Uint8Array }
  | { kind: "snapshot"; term: number; seq: number; data: Uint8Array };

const utf8 = new TextEncoder();

/** A keyboard (or paste) input frame for `term`. */
export function encodeInput(term: number, data: string | Uint8Array): Uint8Array {
  const bytes = typeof data === "string" ? utf8.encode(data) : data;
  const out = new Uint8Array(INPUT_HEADER + bytes.length);
  out[0] = KIND_INPUT;
  new DataView(out.buffer).setUint32(1, term);
  out.set(bytes, INPUT_HEADER);
  return out;
}

/** Decodes a server frame; `null` for anything malformed or unknown. */
export function decodeFrame(buf: ArrayBuffer | Uint8Array): Frame | null {
  const b = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  if (b.length < OUTPUT_HEADER) return null;
  const kind = b[0];
  if (kind !== KIND_OUTPUT && kind !== KIND_SNAPSHOT) return null;
  const v = new DataView(b.buffer, b.byteOffset, b.byteLength);
  const term = v.getUint32(1);
  const seq = Number(v.getBigUint64(5));
  const data = b.subarray(OUTPUT_HEADER);
  return { kind: kind === KIND_OUTPUT ? "output" : "snapshot", term, seq, data };
}
