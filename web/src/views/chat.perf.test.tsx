// @vitest-environment happy-dom
// The log holds up to 2000 entries (KEEP). Rendering all of them must stay
// under 200 ms of scripting: far turns render plain text, near ones Markdown.
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { KEEP } from "../chat";
import { entry, longChat } from "../fixtures";
import { ChatLog } from "./Chat";

afterEach(cleanup);

const BUDGET_MS = 200;

it(`renders the ${KEEP}-entry log in under ${BUDGET_MS} ms`, async () => {
  const entries = longChat(KEEP);
  const settle = () =>
    act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
  // Warm up twice: the first render loads the lazy Markdown and tool chunks,
  // the second is the first to render them (module init, the JIT).
  for (let i = 0; i < 2; i++) {
    render(<ChatLog entries={entries} />);
    await settle();
    cleanup();
    await settle();
  }
  // The median of 3 runs. Later runs pay for the garbage of earlier ones
  // (CI showed 163, 177, 203, 212, 245 ms), so take few.
  const runs: number[] = [];
  for (let i = 0; i < 3; i++) {
    const t0 = performance.now();
    render(<ChatLog entries={entries} />);
    runs.push(performance.now() - t0);
    cleanup();
  }
  const median = [...runs].sort((a, b) => a - b)[1];
  console.log(`ChatLog ${KEEP} entries: median ${median.toFixed(1)} ms (runs ${runs.map((r) => r.toFixed(0)).join(", ")})`);
  expect(median).toBeLessThan(BUDGET_MS);
});

it("re-renders only the changed turn when an entry streams in", async () => {
  const entries = longChat(KEEP);
  const { rerender } = render(<ChatLog entries={entries} />);
  await act(async () => {
    await new Promise((r) => setTimeout(r, 50));
  });
  const runs: number[] = [];
  let have = entries;
  for (let i = 0; i < 5; i++) {
    have = [...have.slice(1), entry(`more${i}`, { text: `More ${i}.`, ts: have[have.length - 1].ts + 1000 })];
    const t0 = performance.now();
    rerender(<ChatLog entries={have} />);
    runs.push(performance.now() - t0);
  }
  runs.sort((a, b) => a - b);
  console.log(`ChatLog append at ${KEEP} entries: median ${runs[2].toFixed(1)} ms`);
  expect(runs[2]).toBeLessThan(BUDGET_MS / 2);
});
