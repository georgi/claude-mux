import { describe, expect, it } from "vitest";
import { decodeFrame, encodeInput } from "./frames";

describe("frames", () => {
  it("encodes input like the Rust side", () => {
    // Frame::Input { term: 1, data: b"x" } encodes to [1, 0, 0, 0, 1, b'x'].
    expect([...encodeInput(1, "x")]).toEqual([1, 0, 0, 0, 1, 0x78]);
    expect([...encodeInput(0x01020304, new Uint8Array([0x1b]))]).toEqual([1, 1, 2, 3, 4, 0x1b]);
  });

  it("decodes output and snapshot frames", () => {
    // The fixed layout from cmux-proto's frame_layout_is_fixed test.
    const out = new Uint8Array([2, 1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 1, 0, 0x68, 0x69]);
    expect(decodeFrame(out)).toEqual({
      kind: "output",
      term: 0x01020304,
      seq: 256,
      data: new Uint8Array([0x68, 0x69]),
    });
    const snap = new Uint8Array([3, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 2]);
    expect(decodeFrame(snap.buffer)).toMatchObject({ kind: "snapshot", term: 7, seq: 2 });
  });

  it("rejects short and unknown frames", () => {
    expect(decodeFrame(new Uint8Array([]))).toBeNull();
    expect(decodeFrame(new Uint8Array([2, 0, 0, 0, 1, 0, 0]))).toBeNull();
    expect(decodeFrame(new Uint8Array(13).fill(0))).toBeNull();
    expect(decodeFrame(encodeInput(1, "xxxxxxxxxx"))).toBeNull();
  });
});
