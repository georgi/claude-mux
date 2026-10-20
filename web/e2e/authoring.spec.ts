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

test("write a draft plan with one task by hand", async ({ page }) => {
  await page.goto(`${server.url}/pair?code=${await server.pairCode()}`);
  await page.getByRole("button", { name: "Pair" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Connected" })).toBeVisible();
  await page.request.post(`${server.url}/v1/repos`, { headers: { "X-Cmux": "1" }, data: { path: server.repo } });

  // The form: a title, a repo from the combobox, a body. No executor message.
  await page.locator("header").first().getByRole("button", { name: "New plan" }).click();
  await page.getByLabel("Title").fill("hand plan");
  await page.getByRole("combobox", { name: "Repositories" }).click();
  await page.getByRole("option", { name: new RegExp(server.repo) }).click();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("list", { name: "Picked repositories" })).toContainText(server.repo);
  await page.getByLabel("Plan body").fill("Write the **docs**.");
  await page.getByRole("tab", { name: "Preview" }).first().click();
  await expect(page.getByText("docs", { exact: true })).toBeVisible();
  if (shots) await page.screenshot({ path: `${shots}/desktop-new-plan.png`, fullPage: true });
  await page.getByRole("button", { name: "Create plan" }).click();

  // The draft board: add one task with a criterion.
  const editor = page.getByRole("region", { name: "Draft tasks" });
  await expect(editor).toBeVisible();
  await editor.getByRole("button", { name: "Add task" }).click();
  const form = page.getByRole("form", { name: "New task" });
  await form.getByRole("button", { name: "Add task" }).click();
  await expect(form.getByText("Give the task a title.")).toBeVisible();
  await form.getByLabel("Task title").fill("Docs");
  await form.getByLabel("Task body").fill("Document the API.");
  await form.getByLabel("Criterion 1", { exact: true }).fill("README covers the API");
  if (shots) await page.screenshot({ path: `${shots}/desktop-task-editor.png`, fullPage: true });
  await form.getByRole("button", { name: "Add task" }).click();
  await expect(form).toBeHidden();

  await expect(editor.getByRole("list", { name: "Draft task order" })).toContainText("Docs");
  await expect(page.locator('[data-column="todo"] [data-task="T-0001"]')).toContainText("Docs");

  // The tree shows the plan as a draft.
  const row = page.getByRole("treeitem", { name: /hand plan/ }).first();
  await expect(row).toContainText(/draft/i);
  const body = await page.request.get(`${server.url}${new URL(page.url()).hash.replace("#/board/", "/v1/plans/")}/body`, { headers: { "X-Cmux": "1" } });
  expect(((await body.json()) as { text: string }).text).toContain("Write the **docs**.");
});
