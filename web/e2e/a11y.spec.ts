import AxeBuilder from "@axe-core/playwright";
import { type BrowserContext, expect, type Page, test } from "@playwright/test";
import { type Cmuxd, startCmuxd } from "./server";

// axe on the seven main views, in the light and the dark theme: zero
// serious or critical violations.

const executor = {
  first: [{ say: "Ready to plan." }],
  later: [
    { when: "run the tests", permission: { tool: "Bash", input: { command: "cargo test -p e2e" } } },
    { when: "run the tests", say: "Tests ran." },
  ],
};

let server: Cmuxd;
let plan = "";
// One pairing for the file: the server limits pairing attempts.
let cookies: Awaited<ReturnType<BrowserContext["cookies"]>> = [];

test.beforeAll(async ({ browser }) => {
  server = await startCmuxd({ executor });
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await page.goto(`${server.url}/pair?code=${await server.pairCode()}`);
  await page.getByRole("button", { name: "Pair" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Connected" })).toBeVisible();
  cookies = await ctx.cookies();
  await ctx.close();
});

test.afterAll(async () => {
  await server?.stop();
});

test.afterEach(async ({}, info) => {
  if (info.status !== info.expectedStatus) await info.attach("cmuxd.log", { body: server.log(), contentType: "text/plain" });
});

async function pair(page: Page) {
  await page.context().addCookies(cookies);
  await page.goto(server.url);
  await expect(page.getByRole("status").filter({ hasText: "Connected" })).toBeVisible();
}

/** One plan for the whole file, made on first use. */
async function ensurePlan(page: Page): Promise<string> {
  if (plan) return plan;
  const h = { "X-Cmux": "1" };
  await page.request.post(`${server.url}/v1/repos`, { headers: h, data: { path: server.repo } });
  const r = await page.request.post(`${server.url}/v1/plans`, {
    headers: h,
    data: { title: "a11y plan", repos: [{ path: server.repo, base_branch: null }], request: "Plan it." },
  });
  expect(r.status()).toBe(201);
  plan = ((await r.json()) as { id: string }).id;
  return plan;
}

/** Runs axe in both color schemes and fails on serious or critical violations. */
async function audit(page: Page, view: string) {
  for (const scheme of ["dark", "light"] as const) {
    await page.emulateMedia({ colorScheme: scheme, reducedMotion: "reduce" });
    await expect(page.locator("html")).toHaveClass(scheme === "dark" ? /\bdark\b/ : /^(?!.*\bdark\b)/);
    const result = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "best-practice"]).analyze();
    const bad = result.violations
      .filter((v) => v.impact === "serious" || v.impact === "critical")
      // An ARIA tree nests each group inside its parent treeitem, so the
      // parent's text includes its children's rows. Its name is its own row's
      // label, as the pattern says; axe's label-in-name check cannot see that.
      .map((v) =>
        v.id === "label-content-name-mismatch"
          ? { ...v, nodes: v.nodes.filter((n) => !/role="treeitem"[^>]*aria-expanded="true"/.test(n.html)) }
          : v,
      )
      .filter((v) => v.nodes.length > 0);
    const text = bad
      .map((v) => `${v.id} (${v.impact}): ${v.help}\n${v.nodes.map((n) => `  ${n.target.join(" ")} — ${n.failureSummary?.split("\n").join(" ")}\n    ${n.html}`).join("\n")}`)
      .join("\n");
    expect(bad.length, `${view} in ${scheme}:\n${text}`).toBe(0);
  }
}

test("pairing", async ({ page }) => {
  await page.goto(`${server.url}/pair`);
  await expect(page.getByRole("button", { name: "Pair" })).toBeVisible();
  await audit(page, "pairing");
});

test("main desktop view", async ({ page }) => {
  await pair(page);
  await ensurePlan(page);
  await expect(page.getByRole("treeitem", { name: "a11y plan" })).toBeVisible();
  await expect(page.getByRole("complementary", { name: "Needs you" })).toBeVisible();
  await audit(page, "main desktop view");
});

test("agent chat", async ({ page }) => {
  await pair(page);
  const id = await ensurePlan(page);
  await page.goto(`${server.url}/#/agent/${encodeURIComponent(`${id}/executor`)}/chat`);
  await expect(page.getByRole("log", { name: "Conversation" })).toContainText("Ready to plan.");
  await audit(page, "agent chat");
});

test("needs-you", async ({ page }) => {
  await pair(page);
  const id = await ensurePlan(page);
  await page.goto(`${server.url}/#/agent/${encodeURIComponent(`${id}/executor`)}/terminal`);
  const screen = page.locator(".xterm-rows");
  await expect(screen).toContainText("TOOLS:");
  await page.locator(".xterm-host").click();
  await page.keyboard.type("run the tests");
  await page.keyboard.press("Enter");
  await page.goto(`${server.url}/#/needs`);
  const card = page.getByRole("main").getByRole("listitem").filter({ hasText: "Allow Bash?" });
  await expect(card).toContainText("cargo test -p e2e");
  await audit(page, "needs-you");
});

test("board", async ({ page }) => {
  await pair(page);
  const id = await ensurePlan(page);
  await page.goto(`${server.url}/#/board/${encodeURIComponent(id)}`);
  await expect(page.getByRole("progressbar", { name: "Plan progress" })).toBeVisible();
  await audit(page, "board");
});

test("settings", async ({ page }) => {
  await pair(page);
  await page.goto(`${server.url}/#/settings`);
  await expect(page.getByRole("heading", { name: "Settings", level: 2 })).toBeVisible();
  await audit(page, "settings");
});

test.describe("on a phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

  test("phone main view", async ({ page }) => {
    await pair(page);
    await ensurePlan(page);
    await expect(page.getByRole("navigation", { name: "Sections" })).toBeVisible();
    await audit(page, "phone main view");
  });
});
