import { expect, test } from "@playwright/test";
import { type Cmuxd, startCmuxd } from "./server";

let server: Cmuxd;

test.beforeAll(async () => {
  server = await startCmuxd({ executor: { first: [{ say: "Ready." }] } });
});

test.afterAll(async () => {
  await server?.stop();
});

test.afterEach(async ({}, info) => {
  if (info.status !== info.expectedStatus) await info.attach("cmuxd.log", { body: server.log(), contentType: "text/plain" });
});

test("settings: this device, theme before first paint, server status", async ({ page }) => {
  await page.goto(`${server.url}/pair?code=${await server.pairCode()}`);
  await page.getByRole("button", { name: "Pair" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Connected" })).toBeVisible();

  await page.goto(`${server.url}/#/settings`);
  const nav = page.getByRole("tablist", { name: "Settings sections" });
  await expect(nav).toBeVisible();
  await expect(page.getByRole("region", { name: "Devices" }).getByText("This device")).toBeVisible();

  // The theme applies at once and survives a reload (main.tsx sets it before React renders).
  await nav.getByRole("tab", { name: "App" }).click();
  const theme = page.getByRole("group", { name: "Theme" });
  await theme.getByRole("button", { name: "Light" }).click();
  await expect(page.locator("html")).not.toHaveClass(/dark/);
  await theme.getByRole("button", { name: "Dark" }).click();
  await expect(page.locator("html")).toHaveClass(/dark/);
  await page.reload();
  await expect(page.locator("html")).toHaveClass(/dark/);
  await expect(page.locator('meta[name="theme-color"]')).toHaveAttribute("content", "#111317");

  await page.getByRole("tablist", { name: "Settings sections" }).getByRole("tab", { name: "Server" }).click();
  const srv = page.getByRole("region", { name: "Server" });
  await expect(srv.getByText("Uptime")).toBeVisible();
  await expect(srv.getByText("Revision")).toBeVisible();
});
