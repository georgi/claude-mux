// Draws the app icon (a `>_` prompt) into public/: icon.svg and the PNGs the
// manifest and iOS need. Run `node scripts/icons.mjs` after a change; the
// output is committed. No dependencies: strokes are rasterized here.

import { writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";

const BG = [0x11, 0x13, 0x17];
const FG = [0x7a, 0xa2, 0xf7];
// On a 512 grid, inside the maskable safe zone (the middle 80%).
const W = 44;
const SEGS = [
  [150, 170, 250, 256],
  [250, 256, 150, 342],
  [290, 342, 370, 342],
];
const out = new URL("../public/", import.meta.url);

function dist(x, y, [ax, ay, bx, by]) {
  const dx = bx - ax, dy = by - ay;
  const t = Math.max(0, Math.min(1, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)));
  return Math.hypot(x - ax - t * dx, y - ay - t * dy);
}

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const c = Buffer.alloc(4);
  c.writeUInt32BE(crc(td));
  return Buffer.concat([len, td, c]);
}

function png(size) {
  const s = 512 / size, N = 4;
  const raw = Buffer.alloc(size * (size * 3 + 1));
  for (let y = 0; y < size; y++) {
    const row = y * (size * 3 + 1);
    for (let x = 0; x < size; x++) {
      let hit = 0;
      for (let j = 0; j < N; j++)
        for (let i = 0; i < N; i++) {
          const px = (x + (i + 0.5) / N) * s, py = (y + (j + 0.5) / N) * s;
          if (SEGS.some((g) => dist(px, py, g) <= W / 2)) hit++;
        }
      const a = hit / (N * N);
      for (let c = 0; c < 3; c++) raw[row + 1 + x * 3 + c] = Math.round(BG[c] + (FG[c] - BG[c]) * a);
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // RGB
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const hex = (c) => `#${c.map((v) => v.toString(16).padStart(2, "0")).join("")}`;
const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">
<rect width="512" height="512" rx="96" fill="${hex(BG)}"/>
<path d="M150 170L250 256L150 342M290 342H370" fill="none" stroke="${hex(FG)}" stroke-width="${W}" stroke-linecap="round" stroke-linejoin="round"/>
</svg>
`;

writeFileSync(new URL("icon.svg", out), svg);
for (const [name, size] of [["icon-192.png", 192], ["icon-512.png", 512], ["apple-touch-icon.png", 180]])
  writeFileSync(new URL(name, out), png(size));
