import { type CDPSession, expect, type Locator, type Page, test } from "@playwright/test";
import { type Cmuxd, startCmuxd } from "./server";

// The phone layout on an iPhone 14 viewport with touch: swipe a permission
// card to answer it, and thumb-sized targets in the tabs and the composer.

const executor = {
  first: [{ say: "Ready to plan." }],
  later: [
    { when: "run the tests", permission: { tool: "Bash", input: { command: "cargo test -p e2e" } } },
    { when: "run the tests", say: "Tests ran." },
  ],
};

let server: Cmuxd;

test.beforeAll(async () => {
  server = await startCmuxd({ executor });
});

test.afterAll(async () => {
  await server?.stop();
});

test.afterEach(async ({}, info) => {
  if (info.status !== info.expectedStatus) await info.attach("cmuxd.log", { body: server.log(), contentType: "text/plain" });
});

test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

const shots = process.env.E2E_SHOTS;

async function pair(page: Page) {
  await page.goto(`${server.url}/pair?code=${await server.pairCode()}`);
  await page.getByRole("button", { name: "Pair" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Connected" })).toBeVisible();
}

async function newPlan(page: Page, title: string): Promise<string> {
  const h = { "X-Cmux": "1" };
  await page.request.post(`${server.url}/v1/repos`, { headers: h, data: { path: server.repo } });
  const r = await page.request.post(`${server.url}/v1/plans`, {
    headers: h,
    data: { title, repos: [{ path: server.repo, base_branch: null }], request: "Plan it." },
  });
  expect(r.status()).toBe(201);
  return ((await r.json()) as { id: string }).id;
}

/** Waits out a route's View Transition: while it runs, the page takes no touches. */
async function viewTransitionDone(page: Page) {
  await page.waitForFunction(() =>
    document.getAnimations().every((a) => !String((a.effect as KeyframeEffect | null)?.pseudoElement ?? "").startsWith("::view-transition")),
  );
}

/** One finger on `el`: down, `dx` px sideways in steps, then (unless `hold`) up. */
async function drag(cdp: CDPSession, el: Locator, dx: number, hold = false) {
  const box = (await el.boundingBox())!;
  const x = box.x + (dx > 0 ? 40 : box.width - 40);
  const y = box.y + 20;
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x, y }] });
  for (let i = 1; i <= 12; i++)
    await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [{ x: x + (dx * i) / 12, y }] });
  if (!hold) await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
}

test("swipe right on a permission card allows it, and the card leaves", async ({ page }) => {
  await pair(page);
  const id = await newPlan(page, "swipe plan");

  // Ask the executor for a permission from its terminal.
  await page.goto(`${server.url}/#/agent/${encodeURIComponent(`${id}/executor`)}/terminal`);
  const screen = page.locator(".xterm-rows");
  await expect(screen).toContainText("TOOLS:");
  await page.locator(".xterm-host").tap();
  await page.keyboard.type("run the tests");
  await page.keyboard.press("Enter");

  const tabs = page.getByRole("navigation", { name: "Sections" });
  await expect(tabs.getByRole("button", { name: "1 Needs you" })).toBeVisible();
  await tabs.getByRole("button", { name: /Needs you$/ }).tap();
  const card = page.getByRole("listitem").filter({ hasText: "Allow Bash?" });
  await expect(card).toContainText("cargo test -p e2e");
  await viewTransitionDone(page);

  // Past the threshold the label under the card turns solid; letting go allows.
  const cdp = await page.context().newCDPSession(page);
  await drag(cdp, card, 160, true);
  const armed = card.locator("[data-armed]");
  await expect(armed).toBeVisible();
  await expect(armed).toContainText("Allow");
  if (shots) await page.screenshot({ path: `${shots}/phone-swipe.png` });
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });

  await expect(card).toHaveCount(0);
  await expect(page.getByText("Nothing needs you")).toBeVisible();
  await expect(tabs.getByRole("button", { name: "Needs you", exact: true })).toBeVisible();
});

test("every button in the bottom tabs and the composer is at least 44x44 px", async ({ page }) => {
  await pair(page);
  const id = await newPlan(page, "sizes plan");
  await page.goto(`${server.url}/#/agent/${encodeURIComponent(`${id}/executor`)}/chat`);
  await expect(page.getByRole("log", { name: "Conversation" })).toContainText("Ready to plan.");

  const tabs = page.getByRole("navigation", { name: "Sections" });
  const composer = page.locator("form").filter({ has: page.getByRole("textbox", { name: "Message" }) });
  await expect(composer).toBeVisible();
  if (shots) await page.screenshot({ path: `${shots}/phone-composer.png` });

  const buttons = [...(await tabs.getByRole("button").all()), ...(await composer.getByRole("button").all())];
  expect(buttons.length).toBeGreaterThanOrEqual(6);
  for (const b of buttons) {
    const box = await b.boundingBox();
    const name = (await b.getAttribute("aria-label")) ?? (await b.textContent());
    expect(box, `${name} has a box`).not.toBeNull();
    expect.soft(box!.width, `${name} width`).toBeGreaterThanOrEqual(44);
    expect.soft(box!.height, `${name} height`).toBeGreaterThanOrEqual(44);
  }

  // The tabs and the key bar keep clear of the screen edges and of each other.
  await page.getByRole("group", { name: "View" }).getByRole("button", { name: "Terminal" }).tap();
  const keys = page.getByRole("toolbar", { name: "Terminal keys" });
  for (const k of ["Esc", "Tab", "Ctrl", "↑", "↓", "←", "→", "|", "~", "/"]) await expect(keys.getByRole("button", { name: k, exact: true })).toBeAttached();
  const esc = (await keys.getByRole("button", { name: "Esc", exact: true }).boundingBox())!;
  expect(esc.height).toBeGreaterThanOrEqual(44);
  if (shots) await page.screenshot({ path: `${shots}/phone-keybar.png` });
});

test("More opens a sheet, and a long-press on a tree row opens its actions", async ({ page }) => {
  await pair(page);
  await newPlan(page, "sheet plan");
  const tabs = page.getByRole("navigation", { name: "Sections" });

  await tabs.getByRole("button", { name: "More" }).tap();
  const more = page.getByRole("dialog");
  await expect(more.getByRole("button", { name: "Settings" })).toBeVisible();
  await more.getByRole("button", { name: "Cancel" }).tap();
  await expect(more).toBeHidden();

  await tabs.getByRole("button", { name: "Agents" }).tap();
  await expect(tabs.getByRole("button", { name: "Agents" })).toHaveAttribute("aria-current", "page");
  const row = page.getByRole("treeitem", { name: /executor/i }).first();
  await expect(row).toBeVisible();
  await viewTransitionDone(page);
  const box = (await row.boundingBox())!;
  const cdp = await page.context().newCDPSession(page);
  const at = { x: box.x + box.width / 2, y: box.y + 22 };
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [at] });
  await page.waitForTimeout(700);
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  const sheet = page.getByRole("dialog");
  await expect(sheet.getByRole("button", { name: "Open chat" })).toBeVisible();
  if (shots) await page.screenshot({ path: `${shots}/phone-row-sheet.png` });
  await sheet.getByRole("button", { name: "Open chat" }).tap();
  await expect(page.getByRole("log", { name: "Conversation" })).toBeVisible();
});
