// For each mark: xterm A = raw bytes[..off]; xterm B = vt100 snapshot (snap-<label>.bin). Compare every cell (char, fg, bg, style) + cursor.
import fs from 'fs'; import pkg from '@xterm/headless'; const { Terminal } = pkg;
const dir = '..'; const bytes = fs.readFileSync(`${dir}/capture.bin`); const m = JSON.parse(fs.readFileSync(`${dir}/marks.json`));
const mk = () => new Terminal({ cols: m.cols, rows: m.rows, allowProposedApi: true, scrollback: 0 });
const feed = (t, b) => new Promise(r => t.write(b, r));
let allOk = true;
for (const mark of m.marks) {
  const A = mk(), B = mk();
  await feed(A, bytes.subarray(0, mark.offset)); await feed(B, fs.readFileSync(`${dir}/snap-${mark.label}.bin`));
  let diffs = 0, ex = [];
  const ca = A.buffer.active.getNullCell(), cb = B.buffer.active.getNullCell();
  for (let y = 0; y < m.rows; y++) {
    const la = A.buffer.active.getLine(y), lb = B.buffer.active.getLine(y);
    for (let x = 0; x < m.cols; x++) {
      const a = la.getCell(x, ca), b = lb.getCell(x, cb);
      const sa = [a.getChars() || ' ', a.getFgColorMode(), a.getFgColor(), a.getBgColorMode(), a.getBgColor(), a.isBold(), a.isItalic(), a.isUnderline(), a.isInverse(), a.isDim(), a.getWidth()].join('|');
      const sb = [b.getChars() || ' ', b.getFgColorMode(), b.getFgColor(), b.getBgColorMode(), b.getBgColor(), b.isBold(), b.isItalic(), b.isUnderline(), b.isInverse(), b.isDim(), b.getWidth()].join('|');
      if (sa !== sb) { diffs++; if (ex.length < 3) ex.push(`(${y},${x}) ${sa} vs ${sb}`); }
    }
  }
  const curSame = A.buffer.active.cursorX === B.buffer.active.cursorX && A.buffer.active.cursorY === B.buffer.active.cursorY;
  const altSame = A.buffer.active.type === B.buffer.active.type;
  console.log(`${mark.label.padEnd(26)} cell_diffs=${diffs} cursor_same=${curSame} (${A.buffer.active.cursorX},${A.buffer.active.cursorY}) buffer=${A.buffer.active.type}/${B.buffer.active.type} ${ex.join(' ; ')}`);
  if (diffs || !curSame || !altSame) allOk = false;
}
console.log('ALL_OK=' + allOk);
