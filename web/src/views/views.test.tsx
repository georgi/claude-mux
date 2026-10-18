// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/preact";
import { afterEach, describe, expect, it } from "vitest";
import type { ItemKind } from "../api/types";
import { agent, entry, fakeApi, item, PLAN, state } from "../fixtures";
import type { Nav } from "../needs";
import { ChatLog, Composer } from "./Chat";
import { NeedsYou } from "./NeedsYou";

afterEach(cleanup);

const nav: Nav = { openAgent: () => {}, openPlan: () => {}, openUrl: () => {} };
const perm: ItemKind = { permission: { tool: "Bash", input_summary: "cargo test -p perms" } };

describe("ChatLog", () => {
  it("renders prompts, text and folded tool calls", () => {
    const { container } = render(
      <ChatLog
        entries={[
          entry("u", { role: "user", text: "Add pagination to /users" }),
          entry("a", { text: "Added cursor pagination and tests." }),
          entry("c", { kind: "tool_call", text: "git push", tool: { name: "Bash", summary: "git push origin" } }),
          entry("r", { role: "user", kind: "tool_result", text: "Everything up-to-date", tool: { name: "Bash", summary: "" } }),
          entry("c2", { kind: "tool_call", text: "", tool: { name: "Bash", summary: "gh pr create" } }),
        ]}
      />,
    );
    const user = container.querySelector(".chat-msg.user");
    expect(user?.textContent).toBe("› Add pagination to /users");
    expect(container.querySelector(".chat-msg.assistant")?.textContent).toBe("Added cursor pagination and tests.");
    const tools = container.querySelectorAll("details.chat-tool");
    expect(tools).toHaveLength(2);
    // The result is folded inside the call, closed until tapped.
    expect(tools[0].querySelector("summary")?.textContent).toContain("Bash git push origin ✓");
    expect(tools[0].querySelector("pre")?.textContent).toBe("Everything up-to-date");
    expect((tools[0] as HTMLDetailsElement).open).toBe(false);
    // A call still running has no result yet.
    expect(tools[1].querySelector("summary")?.textContent).toContain("gh pr create …");
    expect(tools[1].querySelector("pre")).toBeNull();
  });

  it("says when it is loading and when it is empty", () => {
    render(<ChatLog entries={undefined} />);
    expect(screen.getByText("Loading the conversation…")).toBeTruthy();
    cleanup();
    render(<ChatLog entries={[]} />);
    expect(screen.getByText("No messages yet.")).toBeTruthy();
  });
});

describe("Composer", () => {
  it("queues on Enter and steers on the Steer button", async () => {
    const api = fakeApi();
    render(<Composer agent={`${PLAN}/T-0002`} api={api} queued={1} />);
    expect(screen.getByText("✉ 1 queued")).toBeTruthy();
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.input(box, { target: { value: "use cursors" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(api.calls).toEqual([["message", `${PLAN}/T-0002`, "use cursors", false]]));
    await waitFor(() => expect(box.value).toBe(""));
    fireEvent.input(box, { target: { value: "stop, wrong branch" } });
    fireEvent.click(screen.getByText("Steer"));
    await waitFor(() => expect(api.calls[1]).toEqual(["message", `${PLAN}/T-0002`, "stop, wrong branch", true]));
  });
});

describe("NeedsYou", () => {
  const view = (kind: ItemKind, id = 5) =>
    state({ agents: [agent("T-0004")], items: [item(id, { kind, from: `${PLAN}/T-0004`, created: 0 })] });

  it("shows a permission with its actions, and Allow answers it", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view(perm)} api={api} nav={nav} now={120_000} />);
    expect(screen.getByText("Allow Bash?")).toBeTruthy();
    expect(screen.getByText("cargo test -p perms")).toBeTruthy();
    expect(screen.getByText("· 2m")).toBeTruthy();
    expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual(["Allow", "Always", "Deny…", "Terminal"]);
    fireEvent.click(screen.getByText("Allow"));
    await waitFor(() => expect(api.calls).toHaveLength(1));
    const [name, id, req, key] = api.calls[0];
    expect([name, id, req]).toEqual(["answer", 5, { action: "allow", text: null }]);
    expect(key).toMatch(/^[0-9a-f-]{36}$/);
  });

  it("denies with a note", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view(perm)} api={api} nav={nav} />);
    fireEvent.click(screen.getByText("Deny…"));
    fireEvent.input(screen.getByLabelText("Note"), { target: { value: "not on main" } });
    fireEvent.click(screen.getByText("Deny"));
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "deny", text: "not on main" }]));
  });

  it("needs text before a reply goes out", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view("question")} api={api} nav={nav} />);
    fireEvent.click(screen.getByText("Reply…"));
    const send = screen.getByText("Reply") as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    fireEvent.input(screen.getByLabelText("Reply"), { target: { value: "postgres" } });
    expect(send.disabled).toBe(false);
    fireEvent.click(send);
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "reply", text: "postgres" }]));
  });

  it("accepts a plan and resumes a stalled agent", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view("plan_review")} api={api} nav={nav} />);
    fireEvent.click(screen.getByText("Accept"));
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "accept", text: null }]));
    cleanup();
    const api2 = fakeApi();
    render(<NeedsYou view={view("stalled")} api={api2} nav={nav} />);
    fireEvent.click(screen.getByText("Resume"));
    await waitFor(() => expect(api2.calls.map((c) => c[0])).toEqual(["resumeAgent", "answer"]));
    expect(api2.calls[0][1]).toBe(`${PLAN}/T-0004`);
  });

  it("shows the server's error on the card", async () => {
    const api = fakeApi({
      answer: async () => {
        throw new Error("error: I-5 was already answered by c-2");
      },
    });
    render(<NeedsYou view={view(perm)} api={api} nav={nav} />);
    fireEvent.click(screen.getByText("Allow"));
    await waitFor(() => expect(screen.getByText("error: I-5 was already answered by c-2")).toBeTruthy());
  });

  it("disables answers on a read-only device but still opens", () => {
    render(<NeedsYou view={view(perm)} api={fakeApi()} nav={nav} readOnly />);
    expect((screen.getByText("Allow") as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByText("Terminal") as HTMLButtonElement).disabled).toBe(false);
  });

  it("is empty when nothing is pending", () => {
    render(<NeedsYou view={state()} api={fakeApi()} nav={nav} />);
    expect(screen.getByText("Nothing needs you.")).toBeTruthy();
  });
});
