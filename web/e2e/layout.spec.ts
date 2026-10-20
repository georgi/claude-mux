import { expect, test } from "@playwright/test";
import { type Cmuxd, startCmuxd } from "./server";

let server: Cmuxd;

test.beforeAll(async () => {
  server = await startCmuxd({ executor: { first: [{ say: "Ready to plan." }], later: [] } });
});

test.afterAll(async () => {
  await server?.stop();
});

test.afterEach(async ({}, info) => {
  if (info.status !== info.expectedStatus) await info.attach("cmuxd.log", { body: server.log(), contentType: "text/plain" });
});

test.use({ viewport: { width: 1440, height: 900 } });

const shots = process.env.E2E_SHOTS;

test("split shows chat and terminal side by side and is remembered", async ({ page }) => {
  await page.goto(`${server.url}/pair?code=${await server.pairCode()}`);
  await page.getByRole("button", { name: "Pair" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Connected" })).toBeVisible();

  const h = { "X-Cmux": "1" };
  await page.request.post(`${server.url}/v1/repos`, { headers: h, data: { path: server.repo } });
  const r = await page.request.post(`${server.url}/v1/plans`, {
    headers: h,
    data: { title: "layout plan", repos: [{ path: server.repo, base_branch: null }], request: "Plan it." },
  });
  expect(r.status()).toBe(201);

  const { id } = (await r.json()) as { id: string };
  await page.evaluate((plan) => (location.hash = `#/board/${plan}`), id);
  await page.getByRole("button", { name: "Executor", exact: true }).click();

  // The header names where we are.
  const crumbs = page.getByRole("navigation", { name: "breadcrumb" });
  await expect(crumbs.getByRole("link", { name: "layout plan" })).toBeVisible();
  await expect(crumbs).toContainText("Executor");
  await expect(page.getByRole("button", { name: /^Cumulative token usage:/ })).toBeVisible();

  const views = page.getByRole("group", { name: "View" });
  await views.getByRole("button", { name: "Split" }).click();
  await expect(views.getByRole("button", { name: "Split" })).toHaveAttribute("aria-pressed", "true");
  await expect(page).toHaveURL(/\/split$/);
  await expect(page.getByRole("log", { name: "Conversation" })).toContainText("Ready to plan.");
  await expect(page.locator(".xterm")).toBeVisible();
  await expect(page.locator(".xterm-rows")).toContainText("TOOLS:");
  if (shots) await page.screenshot({ path: `${shots}/desktop-split.png` });

  // The choice sticks to the agent: open it without a mode and it is split again.
  const url = new URL(page.url());
  await page.goto(`${server.url}/${url.hash.replace(/\/split$/, "")}`);
  await expect(page.getByRole("group", { name: "View" }).getByRole("button", { name: "Split" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("log", { name: "Conversation" })).toBeVisible();
  await expect(page.locator(".xterm")).toBeVisible();

  // The needs-you column folds to a rail and back.
  await page.getByRole("button", { name: "Collapse needs you" }).click();
  await page.getByRole("button", { name: /^Show needs you/ }).click();
  await expect(page.getByRole("complementary", { name: "Needs you" }).getByRole("heading", { name: "Needs you" })).toBeVisible();
});
