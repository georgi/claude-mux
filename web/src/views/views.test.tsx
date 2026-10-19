// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ItemKind } from "../api/types";
import { agent, entry, fakeApi, item, PLAN, state } from "../fixtures";
import type { Nav } from "../needs";
import { ChatLog, Composer } from "./Chat";
import { NeedsYou } from "./NeedsYou";
import { Pairing } from "./Pairing";

afterEach(cleanup);

const nav: Nav = { openAgent: () => {}, openPlan: () => {}, openUrl: () => {} };
const perm: ItemKind = { permission: { tool: "Bash", input_summary: "cargo test -p perms" } };

describe("ChatLog", () => {
  it("renders prompts, text and folded tool calls", async () => {
    render(
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
    expect(screen.getByText("Add pagination to /users")).toBeTruthy();
    expect(screen.getByText("Added cursor pagination and tests.")).toBeTruthy();
    // The result folds into its call: the output is hidden until the call is tapped.
    const done = screen.getByRole("button", { name: "Bash output" });
    expect(done.getAttribute("aria-expanded")).toBe("false");
    expect(screen.getByText("git push origin")).toBeTruthy();
    expect(screen.queryByText("Everything up-to-date")).toBeNull();
    fireEvent.click(done);
    await waitFor(() => expect(screen.getByText("Everything up-to-date")).toBeTruthy());
    // Only the finished call shows the done mark; the running call has no result yet.
    expect(screen.getAllByLabelText("Done")).toHaveLength(1);
    expect(screen.getByText("gh pr create")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: /output$/ })).toHaveLength(1);
  });

  it("renders a user prompt as an end-aligned bubble", () => {
    render(<ChatLog entries={[entry("u", { role: "user", text: "Fix the login redirect" })]} />);
    const bubble = screen.getByText("Fix the login redirect").closest('[data-slot="bubble"]');
    expect(bubble).not.toBeNull();
    expect(bubble?.getAttribute("data-align")).toBe("end");
  });

  it("renders the assistant reply as markdown", async () => {
    render(<ChatLog entries={[entry("a", { text: "Run **cargo test** first." })]} />);
    // Streamdown parses the markup: the asterisks are gone and the words stay.
    expect(await screen.findByText("cargo test")).toBeTruthy();
    expect(screen.queryByText(/\*\*/)).toBeNull();
  });

  it("renders a tool call as a trace line", () => {
    render(
      <ChatLog entries={[entry("c", { kind: "tool_call", text: "", tool: { name: "Read", summary: "src/chat.ts" } })]} />,
    );
    expect(screen.getByText("Read")).toBeTruthy();
    expect(screen.getByText("src/chat.ts")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Read output" })).toBeNull();
  });

  it("says when it is loading and when it is empty", () => {
    const { unmount } = render(<ChatLog entries={undefined} />);
    expect(screen.getByRole("status", { name: "Loading the conversation" })).toBeTruthy();
    unmount();
    render(<ChatLog entries={[]} />);
    expect(screen.getByText("No messages yet")).toBeTruthy();
  });
});

describe("Composer", () => {
  it("queues on Enter and steers on the Steer button", async () => {
    const api = fakeApi();
    render(<Composer agent={`${PLAN}/T-0002`} api={api} queued={1} />);
    expect(screen.getByText("1 queued")).toBeTruthy();
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.input(box, { target: { value: "use cursors" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(api.calls).toEqual([["message", `${PLAN}/T-0002`, "use cursors", false]]));
    await waitFor(() => expect(box.value).toBe(""));
    fireEvent.input(box, { target: { value: "stop, wrong branch" } });
    fireEvent.click(screen.getByRole("button", { name: "Steer: interrupt the agent" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(api.calls).toHaveLength(1));
    const [name, id, req, key] = api.calls[0];
    expect([name, id, req]).toEqual(["answer", 5, { action: "allow", text: null }]);
    expect(key).toMatch(/^[0-9a-f-]{36}$/);
  });

  it("denies with a note", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view(perm)} api={api} nav={nav} />);
    fireEvent.click(screen.getByRole("button", { name: "Deny…" }));
    fireEvent.input(screen.getByLabelText("Note"), { target: { value: "not on main" } });
    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "deny", text: "not on main" }]));
  });

  it("needs text before a reply goes out", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view("question")} api={api} nav={nav} />);
    fireEvent.click(screen.getByRole("button", { name: "Reply…" }));
    const send = screen.getByRole("button", { name: "Reply" }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    fireEvent.input(screen.getByLabelText("Reply"), { target: { value: "postgres" } });
    expect(send.disabled).toBe(false);
    fireEvent.click(send);
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "reply", text: "postgres" }]));
  });

  it("accepts a plan and resumes a stalled agent", async () => {
    const api = fakeApi();
    render(<NeedsYou view={view("plan_review")} api={api} nav={nav} />);
    fireEvent.click(screen.getByRole("button", { name: "Accept" }));
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "accept", text: null }]));
    cleanup();
    const api2 = fakeApi();
    render(<NeedsYou view={view("stalled")} api={api2} nav={nav} />);
    fireEvent.click(screen.getByRole("button", { name: "Resume" }));
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
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toBe("error: I-5 was already answered by c-2"));
  });

  it("disables answers on a read-only device but still opens", () => {
    render(<NeedsYou view={view(perm)} api={fakeApi()} nav={nav} readOnly />);
    expect((screen.getByRole("button", { name: "Allow" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Terminal" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("is empty when nothing is pending", () => {
    render(<NeedsYou view={state()} api={fakeApi()} nav={nav} />);
    expect(screen.getByText("Nothing needs you.")).toBeTruthy();
  });

  it("shows an empty state with no items when only answered items remain", () => {
    const answered = state({
      agents: [agent("T-0004")],
      items: [item(7, { kind: perm, from: `${PLAN}/T-0004`, status: { answered: { by: "c1", answer: "y" } } })],
    });
    render(<NeedsYou view={answered} api={fakeApi()} nav={nav} />);
    expect(screen.getByText("All clear")).toBeTruthy();
    expect(screen.queryByRole("list", { name: "Needs you" })).toBeNull();
    expect(screen.queryAllByRole("listitem")).toHaveLength(0);
  });
});

describe("Pairing", () => {
  it("enables Pair only after 8 code characters", () => {
    render(<Pairing onPaired={() => {}} />);
    const pair = screen.getByRole("button", { name: "Pair" }) as HTMLButtonElement;
    const code = screen.getByLabelText("Pairing code") as HTMLInputElement;
    expect(pair.disabled).toBe(true);
    fireEvent.change(code, { target: { value: "ABCD123" } });
    expect(pair.disabled).toBe(true);
    fireEvent.change(code, { target: { value: "ABCD1234" } });
    expect(pair.disabled).toBe(false);
  });
});
