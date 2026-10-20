// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DeviceView, StatusResp } from "../api/types";
import { fakeApi } from "../fixtures";
import { Settings } from "./Settings";
import { browserOf, pushHint } from "./settings/notifications";
import { byRss } from "./settings/server";
import { getTheme, setTheme, THEME_KEY } from "./settings/theme";

afterEach(cleanup);
beforeEach(() => {
  localStorage.clear();
  document.documentElement.classList.remove("dark");
});

const open = (name: string) => fireEvent.click(screen.getByRole("tab", { name }));

const device = (id: string, name: string, over: Partial<DeviceView> = {}): DeviceView => ({
  id,
  name,
  scope: "full",
  created: Date.now() - 86_400_000 * 3,
  last_seen: Date.now() - 120_000,
  current: false,
  ...over,
});

describe("Settings theme", () => {
  it("switches the class on <html> and stores the choice", async () => {
    render(<Settings api={fakeApi()} />);
    open("App");
    const group = await screen.findByRole("group", { name: "Theme" });
    fireEvent.click(within(group).getByRole("button", { name: "Dark" }));
    await waitFor(() => expect(document.documentElement.classList.contains("dark")).toBe(true));
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    fireEvent.click(within(group).getByRole("button", { name: "Light" }));
    await waitFor(() => expect(document.documentElement.classList.contains("dark")).toBe(false));
    expect(localStorage.getItem(THEME_KEY)).toBe("light");
    expect(await screen.findByText("Saved")).toBeTruthy();
    // System drops the key, so the OS scheme decides again.
    fireEvent.click(within(group).getByRole("button", { name: "System" }));
    await waitFor(() => expect(localStorage.getItem(THEME_KEY)).toBeNull());
    expect(getTheme()).toBe("system");
  });

  it("follows setTheme from outside the view (the palette)", async () => {
    render(<Settings api={fakeApi()} />);
    open("App");
    const dark = within(await screen.findByRole("group", { name: "Theme" })).getByRole("button", { name: "Dark" });
    expect(dark.getAttribute("aria-pressed")).toBe("false");
    setTheme("dark");
    await waitFor(() => expect(dark.getAttribute("aria-pressed")).toBe("true"));
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });
});

describe("Settings devices", () => {
  it("marks this device and revokes another after an AlertDialog naming it", async () => {
    const api = fakeApi({
      devices: async () => ({ devices: [device("d1", "Work laptop", { current: true }), device("d2", "Old iPad", { scope: "read" })] }),
    });
    render(<Settings api={api} />);
    expect(await screen.findByText("This device")).toBeTruthy();
    expect(screen.getByText("Read only")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Revoke Old iPad" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Revoke “Old iPad”?")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Revoke device" }));
    await waitFor(() => expect(api.calls).toContainEqual(["revokeDevice", "d2"]));
    expect(await screen.findByText("Saved")).toBeTruthy();
  });

  it("retries a failed device load", async () => {
    let fail = true;
    const api = fakeApi({
      devices: async () => {
        if (fail) throw new Error("server down");
        return { devices: [device("d1", "Work laptop", { current: true })] };
      },
    });
    render(<Settings api={api} />);
    expect(await screen.findByText("server down")).toBeTruthy();
    fail = false;
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("This device")).toBeTruthy();
    expect(screen.queryByText("server down")).toBeNull();
  });

  it("signs out after revoking this device", async () => {
    const onSignedOut = vi.fn();
    const api = fakeApi({ devices: async () => ({ devices: [device("d1", "Work laptop", { current: true })] }) });
    render(<Settings api={api} onSignedOut={onSignedOut} />);
    fireEvent.click(await screen.findByRole("button", { name: "Revoke Work laptop" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Revoke “Work laptop”?")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Revoke and sign out" }));
    await waitFor(() => expect(onSignedOut).toHaveBeenCalled());
    expect(api.calls).toContainEqual(["revokeDevice", "d1"]);
  });

  it("shows a failed revoke inline", async () => {
    const api = fakeApi({
      devices: async () => ({ devices: [device("d2", "Old iPad")] }),
      revokeDevice: async () => {
        throw new Error("server said no");
      },
    });
    render(<Settings api={api} />);
    fireEvent.click(await screen.findByRole("button", { name: "Revoke Old iPad" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Revoke device" }));
    expect(await screen.findByText(/server said no/)).toBeTruthy();
  });
});

describe("Settings server", () => {
  const status: StatusResp = {
    server: { name: "cmuxd", version: "1.2.3" },
    uptime_secs: 90_000,
    rev: 42,
    load: null,
    live_processes: 3,
    clients: 1,
    agents: [
      { agent: "P-1/T-0001", rss_kb: 100 * 1024 },
      { agent: "P-1/T-0002", rss_kb: null },
      { agent: "P-1/executor", rss_kb: 400 * 1024 },
      { agent: "P-1/T-0003", rss_kb: 250 * 1024 },
    ],
    github: "on",
  };

  it("lists per-agent RSS largest first", async () => {
    render(<Settings api={fakeApi({ status: async () => status })} />);
    open("Server");
    const list = await screen.findByRole("list", { name: "Memory by agent" });
    const rows = within(list).getAllByRole("listitem").map((li) => li.textContent);
    expect(rows).toEqual(["executor400 MB", "T-0003250 MB", "T-0001100 MB", "T-0002n/a"]);
    expect(screen.getByText("cmuxd 1.2.3")).toBeTruthy();
    expect(screen.getByText("1d 1h")).toBeTruthy();
    expect(screen.getByText("42")).toBeTruthy();
  });

  it("sorts without a reading last", () => {
    expect(byRss(status.agents).map((a) => a.rss_kb)).toEqual([409600, 256000, 102400, null]);
  });
});

describe("Settings layout", () => {
  it("has a nav with every section, each a titled card", async () => {
    render(<Settings api={fakeApi()} />);
    const tabs = screen.getAllByRole("tab").map((t) => t.textContent?.trim());
    expect(tabs).toEqual(["Devices", "Notifications", "Repositories", "Limits", "Claude", "App", "Server"]);
    open("Limits");
    expect(await screen.findByRole("region", { name: "Limits" })).toBeTruthy();
    expect(screen.getByText("Live Claude processes")).toBeTruthy();
  });
});

describe("push hints", () => {
  it("tells iOS to install first and each browser how to unblock", () => {
    const iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Version/18.0 Mobile/15E148 Safari/604.1";
    const safari = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
    const chrome = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";
    const firefox = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14.0; rv:140.0) Gecko/20100101 Firefox/140.0";
    expect([iphone, safari, chrome, firefox].map((ua) => browserOf(ua))).toEqual(["ios", "safari", "chromium", "firefox"]);
    expect(pushHint("unsupported", "ios", false)).toMatch(/Add to Home Screen/);
    expect(pushHint("denied", "ios", true)).toMatch(/iOS Settings/);
    expect(pushHint("denied", "safari", false)).toMatch(/Safari → Settings/);
    expect(pushHint("denied", "chromium", false)).toMatch(/site icon/);
  });
});
