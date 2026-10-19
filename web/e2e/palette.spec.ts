import { expect, test } from "@playwright/test";
import { type Cmuxd, startCmuxd } from "./server";

const shots = process.env.E2E_SHOTS;

// A server of its own: one plan, so one agent answers to `executor`.
let server: Cmuxd;

test.beforeAll(async () => {
  server = await startCmuxd({ executor: { first: [{ say: "Ready to plan." }] } });
});

test.afterAll(async () => {
  await server?.stop();
});

test.afterEach(async ({}, info) => {
  if (info.status !== info.expectedStatus) await info.attach("cmuxd.log", { body: server.log(), contentType: "text/plain" });
});

test("the palette opens an agent and ? lists the shortcuts", async ({ page }) => {
  await page.goto(`${server.url}/pair?code=${await server.pairCode()}`);
  await page.getByRole("button", { name: "Pair" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Connected" })).toBeVisible();

  const h = { "X-Cmux": "1" };
  await page.request.post(`${server.url}/v1/repos`, { headers: h, data: { path: server.repo } });
  const r = await page.request.post(`${server.url}/v1/plans`, {
    headers: h,
    data: { title: "palette plan", repos: [{ path: server.repo, base_branch: null }], request: "Plan it." },
  });
  expect(r.status()).toBe(201);
  const tree = page.getByRole("navigation", { name: "Agents and plans" });
  await expect(tree.getByText("executor")).toBeVisible();

  // Control+K opens the palette; the agent's path finds it; Enter opens it.
  await page.locator("body").click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("Control+K");
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await expect(palette).toBeVisible();
  await palette.getByRole("combobox", { name: "Command" }).fill("executor");
  await expect(palette.getByRole("option").first()).toContainText("executor");
  if (shots) await page.waitForTimeout(400).then(() => page.screenshot({ path: `${shots}/desktop-palette.png` }));
  await page.keyboard.press("Enter");
  await expect(palette).toBeHidden();
  await expect(page.getByRole("heading", { level: 2, name: "executor" })).toBeVisible();
  await expect(page).toHaveURL(/#\/agent\/.+\/executor/);

  // `c` switches to chat; `?` lists every binding.
  await page.locator("main").first().click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("c");
  await expect(page.getByRole("group", { name: "View" }).getByRole("button", { name: "Chat" })).toHaveAttribute("aria-pressed", "true");
  await page.locator("main").first().click({ position: { x: 5, y: 5 } });
  await page.keyboard.press("?");
  const sheet = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(sheet).toContainText("Needs you");
  await expect(sheet).toContainText("Next agent");
  if (shots) await page.waitForTimeout(400).then(() => page.screenshot({ path: `${shots}/desktop-shortcuts.png` }));
  await page.keyboard.press("Escape");
  await expect(sheet).toBeHidden();

  // `g s` goes to settings.
  await page.keyboard.press("g");
  await page.keyboard.press("s");
  await expect(page).toHaveURL(/#\/settings$/);
});
