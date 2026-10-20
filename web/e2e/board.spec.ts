import { expect, test } from "@playwright/test";
import { type Cmuxd, startCmuxd } from "./server";

let server: Cmuxd;
const shots = process.env.E2E_SHOTS;

test.beforeAll(async () => {
  server = await startCmuxd({ executor: { first: [{ say: "Ready to plan." }] } });
});

test.afterAll(async () => {
  await server?.stop();
});

test.afterEach(async ({}, info) => {
  if (info.status !== info.expectedStatus) await info.attach("cmuxd.log", { body: server.log(), contentType: "text/plain" });
});

test("board views, dependency graph and the task sheet", async ({ page }) => {
  await page.goto(`${server.url}/pair?code=${await server.pairCode()}`);
  await page.getByRole("button", { name: "Pair" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Connected" })).toBeVisible();

  // A plan with two tasks, the second waiting on the first.
  const h = { "X-Cmux": "1" };
  await page.request.post(`${server.url}/v1/repos`, { headers: h, data: { path: server.repo } });
  const r = await page.request.post(`${server.url}/v1/plans`, {
    headers: h,
    data: { title: "board plan", repos: [{ path: server.repo, base_branch: null }] },
  });
  expect(r.status()).toBe(201);
  const { id } = (await r.json()) as { id: string };
  const add = (data: object) => page.request.post(`${server.url}/v1/plans/${id}/tasks`, { headers: h, data });
  expect((await add({ title: "Schema", body: "Add the **users** table.", criteria: ["migration runs", "docs"] })).status()).toBe(201);
  expect((await add({ title: "API", deps: ["T-0001"] })).status()).toBe(201);

  await page.goto(`${server.url}/#/board/${id}`);
  await expect(page.getByText("0/2 merged")).toBeVisible();
  await expect(page.locator('[data-column="todo"] [data-task]')).toHaveCount(2);
  await expect(page.locator('[data-task="T-0002"]')).toContainText("waits on T-0001");
  if (shots) await page.screenshot({ path: `${shots}/desktop-board-columns.png` });

  // The graph: two nodes, one edge; a node opens the task.
  await page.getByRole("button", { name: "Graph" }).click();
  const graph = page.getByRole("group", { name: "Task dependency graph" });
  await expect(graph.locator("[data-edge]")).toHaveCount(1);
  if (shots) await page.screenshot({ path: `${shots}/desktop-board-graph.png` });
  await graph.getByRole("button", { name: /^T-0001 Schema/ }).click();

  const sheet = page.getByRole("dialog");
  await expect(sheet).toContainText("Schema");
  await expect(sheet.getByRole("list", { name: "Criteria list" })).toContainText("migration runs");
  await expect(sheet.getByText("users", { exact: true })).toBeVisible();
  await expect(sheet.getByText("No notes yet.")).toBeVisible();
  if (shots) await page.screenshot({ path: `${shots}/desktop-task-sheet.png` });
  await page.keyboard.press("Escape");
  await expect(sheet).toBeHidden();

  // The choice of view stays with the plan.
  await page.reload();
  await expect(page.getByRole("group", { name: "Task dependency graph" })).toBeVisible();
});
