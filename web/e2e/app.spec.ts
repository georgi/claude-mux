import { expect, test } from "@playwright/test";
import { type Cmuxd, startCmuxd } from "./server";

// The executor greets, then on "run the tests" asks for a Bash permission
// and reports what it got.
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

async function pair(page: import("@playwright/test").Page) {
  await page.goto(`${server.url}/pair?code=${await server.pairCode()}`);
  await page.getByRole("button", { name: "Pair" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Connected" })).toBeVisible();
}

const shots = process.env.E2E_SHOTS;

test("pair, open an agent, type into its terminal, approve its permission", async ({ page }) => {
  // Pair with a code from the server.
  await pair(page);

  // A plan on a repository added by its path on the server.
  await page.locator("header").first().getByRole("button", { name: "New plan" }).click();
  await page.getByLabel("Title").fill("e2e plan");
  await page.getByLabel("Repository URL or path").fill(server.repo);
  await page.getByRole("button", { name: "Add", exact: true }).click();
  await expect(page.getByRole("checkbox", { name: server.repo })).toBeChecked();
  await page.getByLabel("First message for the executor").fill("Plan a small change.");
  await page.getByRole("button", { name: "Create plan" }).click();

  // The board opens; open the executor from it.
  await expect(page.getByRole("progressbar", { name: "Plan progress" })).toBeVisible();
  await expect(page.getByText("e2e plan").first()).toBeVisible();
  await page.getByRole("button", { name: "Executor", exact: true }).click();
  const screen = page.locator(".xterm-rows");
  await expect(screen).toContainText("TOOLS:");

  // Type into the terminal: the fake claude takes the line as a prompt.
  await page.locator(".xterm-host").click();
  await page.keyboard.type("run the tests");
  await page.keyboard.press("Enter");
  await expect(screen).toContainText("ECHO run the tests");

  // The permission lands in needs-you; Allow lets the agent go on.
  const needs = page.getByRole("complementary", { name: "Needs you" });
  const card = needs.getByRole("listitem").filter({ hasText: "Allow Bash?" });
  await expect(card).toContainText("cargo test -p e2e");
  if (shots) await page.screenshot({ path: `${shots}/desktop-permission.png` });
  await card.getByRole("button", { name: "Allow", exact: true }).click();
  await expect(screen).toContainText("PERM allow");
  await expect(needs).toContainText("Nothing needs you.");

  // The chat shows the same turn.
  await page.getByRole("group", { name: "View" }).getByRole("button", { name: "Chat" }).click();
  await expect(page.getByRole("log", { name: "Conversation" })).toContainText("Tests ran.");
  await expect(page.getByRole("log", { name: "Conversation" })).toContainText("run the tests");
});

test.describe("on a phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

  test("tabs, chat first, and the terminal key bar", async ({ page }) => {
    await pair(page);
    const h = { "X-Cmux": "1" };
    await page.request.post(`${server.url}/v1/repos`, { headers: h, data: { path: server.repo } });
    const r = await page.request.post(`${server.url}/v1/plans`, {
      headers: h,
      data: { title: "phone plan", repos: [{ path: server.repo, base_branch: null }], request: "Plan it." },
    });
    expect(r.status()).toBe(201);

    const tabs = page.getByRole("navigation", { name: "Sections" });
    await expect(tabs).toBeVisible();
    await expect(page.getByRole("navigation", { name: "Agents and plans" })).toBeHidden();
    await tabs.getByRole("button", { name: "Plans" }).click();
    await page.getByRole("button", { name: /phone plan/ }).click();
    if (shots) await page.screenshot({ path: `${shots}/phone-board.png` });
    await page.getByRole("button", { name: "Executor", exact: true }).click();

    // Chat is the default agent view on a phone.
    await expect(page.getByRole("group", { name: "View" }).getByRole("button", { name: "Chat" })).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByRole("log", { name: "Conversation" })).toContainText("Ready to plan.");
    if (shots) await page.screenshot({ path: `${shots}/phone-chat.png` });

    // The key bar types into the terminal.
    await page.getByRole("group", { name: "View" }).getByRole("button", { name: "Terminal" }).click();
    const keys = page.getByRole("toolbar", { name: "Terminal keys" });
    for (const k of ["Esc", "Tab", "Ctrl", "↑", "↓", "←", "→"]) await expect(keys.getByRole("button", { name: k, exact: true })).toBeVisible();
    await keys.getByRole("button", { name: "2", exact: true }).click();
    await keys.getByRole("button", { name: "⏎", exact: true }).click();
    await expect(page.locator(".xterm-rows")).toContainText("ECHO 2");
    if (shots) await page.screenshot({ path: `${shots}/phone-terminal.png` });
  });
});
