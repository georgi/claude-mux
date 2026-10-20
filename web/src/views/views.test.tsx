// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ChatEntry, ItemKind } from "../api/types";
import { agent, entry, fakeApi, item, PLAN, state } from "../fixtures";
import type { Nav } from "../needs";
import { ChatLog, Composer } from "./Chat";
import { PendingRequest, pendingFor } from "./chat/Pending";
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

describe("tool cards", () => {
  const call = (uuid: string, name: string, input: Record<string, unknown>, summary = name) =>
    entry(uuid, { kind: "tool_call", text: "", tool: { name, summary, input: input as never }, call_id: `id-${uuid}`, ts: 0 });
  const result = (uuid: string, name: string, text: string, over: Partial<ChatEntry> = {}) =>
    entry(`r-${uuid}`, { role: "user", kind: "tool_result", text, tool: { name, summary: "" }, call_id: `id-${uuid}`, ...over });

  it("shows a failed Bash call in red, with its error output as an alert and the full output on demand", async () => {
    const api = fakeApi({ entry: async () => entry("full", { text: "error: linker failed\nfull tail" }) });
    render(
      <ChatLog
        tools={{ agent: `${PLAN}/T-0003`, api }}
        entries={[
          call("b", "Bash", { command: "cargo build", description: "Build the workspace" }),
          result("b", "Bash", "error: linker failed\n  at main (src/main.rs:3:1)", { is_error: true, full_len: 5000 }),
        ]}
      />,
    );
    expect(await screen.findByText("Build the workspace")).toBeTruthy();
    expect(screen.getByText("$ cargo build")).toBeTruthy();
    expect(screen.getByLabelText("Error")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Bash output" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("error: linker failed");
    fireEvent.click(screen.getByRole("button", { name: /Show full output/ }));
    await waitFor(() => expect(screen.getByText("full tail")).toBeTruthy());
  });

  it("keeps every line of a Bash command accessible while running and after completion", async () => {
    const command = "python3 - <<'PY'\nprint('second line')\nPY";
    const bash = call("b", "Bash", { command });
    const { rerender } = render(<ChatLog entries={[call("single", "Bash", { command: "pwd" })]} />);
    await screen.findByText("$ pwd");
    expect(screen.queryByRole("button", { name: "Bash output" })).toBeNull();
    rerender(<ChatLog entries={[bash]} />);
    const toggle = await screen.findByRole("button", { name: "Bash output" });
    fireEvent.click(toggle);
    await waitFor(() => expect(screen.getByLabelText("Bash command").textContent).toBe(command));
    expect(screen.getByRole("button", { name: "Copy command" })).toBeTruthy();

    rerender(<ChatLog entries={[bash, result("b", "Bash", "second line")]} />);
    await waitFor(() => expect(screen.getByText("second line")).toBeTruthy());
    expect(screen.getByLabelText("Bash command").textContent).toBe(command);
  });

  it("shows an Edit with its +/- counts and the diff", async () => {
    render(
      <ChatLog
        tools={{ agent: "a", cwd: "/repo" }}
        entries={[
          call("e", "Edit", { file_path: "/repo/src/chat.ts", old_string: "a\nb\nc", new_string: "a\nB\nB2\nc" }),
          result("e", "Edit", "The file was updated."),
        ]}
      />,
    );
    const counts = await screen.findByLabelText("2 added, 1 removed");
    expect(counts.textContent).toBe("+2 −1");
    expect(screen.getByText("chat.ts")).toBeTruthy();
    expect(screen.getByText("src/")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Edit output" }));
    await waitFor(() => expect(screen.getByLabelText("Changes to /repo/src/chat.ts")).toBeTruthy());
    expect(document.querySelectorAll('[data-op="+"]')).toHaveLength(2);
    expect(document.querySelectorAll('[data-op="-"]')).toHaveLength(1);
  });

  it("marks capped Edit input as partial without claiming complete change counts", async () => {
    // The actual edit differs only after the identical 20 KiB prefixes retained by the server.
    const prefix = "x\n".repeat(10240);
    render(<ChatLog entries={[
      call("e", "Edit", { file_path: "/repo/a.txt", old_string: prefix, new_string: prefix, truncated: true }),
      result("e", "Edit", "The file was updated."),
    ]} />);
    expect(await screen.findByText("Partial input")).toBeTruthy();
    expect(screen.queryByLabelText(/added, .*removed/)).toBeNull();
  });

  it("marks truncated Write content before the preview is opened", async () => {
    render(<ChatLog entries={[
      call("w", "Write", { file_path: "/repo/a.txt", content: "partial", truncated: true }),
      result("w", "Write", "The file was written."),
    ]} />);
    expect(await screen.findByText("Partial input")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Write output" }).getAttribute("aria-expanded")).toBe("false");
  });

  it("shows a Task with its sub-agent's state and a button that opens it", async () => {
    const opened: string[] = [];
    const parent = `${PLAN}/T-0003`;
    const sub = { ...agent("T-0003/lint"), level: "sub" as const, parent, activity: "working" as const };
    render(
      <ChatLog
        tools={{ agent: parent, agents: [agent("T-0003"), sub], onOpenAgent: (id) => opened.push(id) }}
        entries={[call("t", "Task", { description: "Run lint on the web app", subagent_type: "general-purpose" })]}
      />,
    );
    expect(await screen.findByText("Run lint on the web app")).toBeTruthy();
    expect(screen.getByText("general-purpose")).toBeTruthy();
    expect(screen.getByText("Working")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Open agent lint" }));
    expect(opened).toEqual([sub.id]);
  });

  it("opens only the latest todo list, and folds three tool calls into one step group", async () => {
    const todos = (s: string) => ({ todos: [{ content: `Write ${s}`, status: "completed" }, { content: "Test", status: "in_progress", activeForm: "Testing" }] });
    render(
      <ChatLog
        entries={[
          call("t1", "TodoWrite", todos("old")),
          result("t1", "TodoWrite", "ok"),
          entry("x", { text: "Next." }),
          call("g1", "Read", { file_path: "/a.ts" }),
          call("g2", "Grep", { pattern: "foo" }),
          call("t2", "TodoWrite", todos("new")),
          result("t2", "TodoWrite", "ok"),
        ]}
      />,
    );
    // The group still runs (two calls have no result), so it stays open.
    expect(await screen.findByText(/^Working · 3 steps/)).toBeTruthy();
    expect(await screen.findByText("Write new")).toBeTruthy();
    expect(screen.queryByText("Write old")).toBeNull();
  });

  it("falls back to the summary line without structured input", () => {
    render(<ChatLog entries={[entry("c", { kind: "tool_call", text: "", tool: { name: "Edit", summary: "src/a.rs" } })]} />);
    expect(screen.getByText("Edit")).toBeTruthy();
    expect(screen.getByText("src/a.rs")).toBeTruthy();
  });
});

describe("Composer", () => {
  afterEach(() => localStorage.clear());

  it("queues on Enter and steers with the Steer toggle or Cmd+Enter", async () => {
    const api = fakeApi();
    render(<Composer agent={`${PLAN}/T-0002`} api={api} queued={[{ id: 1, text: "also add tests", steer: false }]} />);
    const queue = screen.getByRole("region", { name: "Queued messages" });
    expect(queue.textContent).toContain("1 queued");
    expect(queue.textContent).toContain("also add tests");
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(box, { target: { value: "use cursors" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(api.calls).toEqual([["message", `${PLAN}/T-0002`, "use cursors", false]]));
    await waitFor(() => expect(box.value).toBe(""));
    fireEvent.change(box, { target: { value: "stop, wrong branch" } });
    fireEvent.click(screen.getByRole("button", { name: "Steer: interrupt the agent" }));
    fireEvent.click(screen.getByRole("button", { name: "Steer now" }));
    await waitFor(() => expect(api.calls[1]).toEqual(["message", `${PLAN}/T-0002`, "stop, wrong branch", true]));
    await waitFor(() => expect(box.value).toBe(""));
    fireEvent.change(box, { target: { value: "now" } });
    fireEvent.keyDown(box, { key: "Enter", metaKey: true });
    await waitFor(() => expect(api.calls[2]).toEqual(["message", `${PLAN}/T-0002`, "now", true]));
  });

  it("keeps a draft per agent and restores it on return", async () => {
    const a = `${PLAN}/T-0001`;
    const b = `${PLAN}/T-0002`;
    const api = fakeApi();
    const { rerender, unmount } = render(<Composer agent={a} api={api} queued={[]} />);
    const box = () => screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(box(), { target: { value: "half a thought for A" } });
    rerender(<Composer agent={b} api={api} queued={[]} />);
    expect(box().value).toBe("");
    fireEvent.change(box(), { target: { value: "B's draft" } });
    rerender(<Composer agent={a} api={api} queued={[]} />);
    expect(box().value).toBe("half a thought for A");
    // A reload keeps it too.
    unmount();
    render(<Composer agent={b} api={api} queued={[]} />);
    expect(box().value).toBe("B's draft");
    // Sending clears it.
    fireEvent.keyDown(box(), { key: "Enter" });
    await waitFor(() => expect(box().value).toBe(""));
    expect(localStorage.getItem(`cmux.draft.${b}`)).toBeNull();
  });

  it("keeps slash-prefixed messages as plain text without offering native commands", async () => {
    const api = fakeApi();
    render(<Composer agent={`${PLAN}/T-0002`} api={api} queued={[]} />);
    const box = screen.getByLabelText("Message") as HTMLTextAreaElement;
    fireEvent.change(box, { target: { value: "/co" } });
    expect(screen.queryByRole("listbox", { name: "Commands" })).toBeNull();
    fireEvent.keyDown(box, { key: "Tab" });
    expect(box.value).toBe("/co");
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(api.calls).toEqual([["message", `${PLAN}/T-0002`, "/co", false]]));
  });

  it("says why it is disabled on a read-only device", () => {
    render(<Composer agent={`${PLAN}/T-0002`} api={fakeApi()} queued={[]} readOnly />);
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Send at the next turn" }) as HTMLButtonElement).disabled).toBe(true);
  });
});

describe("chat rhythm", () => {
  const day = (d: number, h: number) => new Date(2025, 9, d, h).getTime();

  it("puts a day line where the date changes and one header over Claude's turn", () => {
    render(
      <ChatLog
        entries={[
          entry("u1", { role: "user", text: "Start", ts: day(18, 9) }),
          entry("a1", { text: "Started.", ts: day(18, 9) + 1000, model: "claude-opus-5-5" }),
          entry("a2", { text: "Still going.", ts: day(18, 9) + 2000 }),
          entry("u2", { role: "user", text: "Next day", ts: day(19, 10) }),
          entry("a3", { text: "Good morning.", ts: day(19, 10) + 1000 }),
        ]}
      />,
    );
    const lines = screen.getAllByRole("separator").map((s) => s.getAttribute("aria-label"));
    expect(lines).toHaveLength(2);
    expect(lines[0]).toBe("Sat, Oct 18, 2025");
    expect(lines[1]).toBe("Sun, Oct 19, 2025");
    // a1 and a2 form one turn with one header.
    expect(screen.getAllByText("Claude")).toHaveLength(2);
    expect(screen.getByText("opus-5-5")).toBeTruthy();
    // Each turn carries its time, with the absolute time for a tooltip.
    expect(document.querySelectorAll("time")).toHaveLength(4);
  });

  it("folds thinking by default under \"Thought for Ns\"", async () => {
    render(
      <ChatLog
        entries={[
          entry("t", { kind: "thinking", text: "Maybe the cache is stale.", ts: 0 }),
          entry("a", { text: "Clearing the cache.", ts: 4000 }),
        ]}
      />,
    );
    const trigger = screen.getByRole("button", { name: /Thought for 4s/ });
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("Maybe the cache is stale.")).toBeNull();
    fireEvent.click(trigger);
    await waitFor(() => expect(screen.getByText("Maybe the cache is stale.")).toBeTruthy());
  });

  it("shows what the agent does under the log", () => {
    const { rerender } = render(<ChatLog entries={[entry("a", { text: "Hi" })]} activity="working" />);
    expect(screen.getByRole("status", { name: "Agent activity" }).textContent).toBe("Working…");
    rerender(<ChatLog entries={[entry("a", { text: "Hi" })]} activity="needs_permission" />);
    expect(screen.getByRole("status", { name: "Agent activity" }).textContent).toBe("Waiting for permission");
    rerender(<ChatLog entries={[entry("a", { text: "Hi" })]} activity="idle" idleSince={Date.now() - 4 * 60_000} />);
    expect(screen.getByRole("status", { name: "Agent activity" }).textContent).toBe("Idle 4m");
  });

  it("offers copy and quote actions; quote fills the composer", async () => {
    const a = `${PLAN}/T-0002`;
    render(
      <>
        <ChatLog tools={{ agent: a }} entries={[entry("x", { text: "Use **cursors**." })]} />
        <Composer agent={a} api={fakeApi()} queued={[]} />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Quote in reply" }));
    await waitFor(() => expect((screen.getByLabelText("Message") as HTMLTextAreaElement).value).toBe("> Use **cursors**.\n\n"));
    expect(screen.getByRole("button", { name: "Copy text" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Copy as Markdown" })).toBeTruthy();
  });
});

describe("PendingRequest", () => {
  it("shows a permission as a confirmation with its input as code", async () => {
    const api = fakeApi();
    const v = state({ agents: [agent("T-0004")], items: [item(5, { kind: perm, from: `${PLAN}/T-0004` })] });
    render(<PendingRequest need={pendingFor(v, `${PLAN}/T-0004`)[0]} api={api} />);
    const card = screen.getByRole("group", { name: "Allow Bash?" });
    expect(card.querySelector('[data-slot="code-block"]')?.textContent).toBe("cargo test -p perms");
    expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual(["Deny", "Always", "Allow"]);
    fireEvent.click(screen.getByRole("button", { name: "Always" }));
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 5, { action: "allow_always", text: null }]));
  });

  it("sends a reply to a question with Cmd+Enter", async () => {
    const api = fakeApi();
    const v = state({ agents: [agent("T-0004")], items: [item(6, { from: `${PLAN}/T-0004` })] });
    render(<PendingRequest need={pendingFor(v, `${PLAN}/T-0004`)[0]} api={api} />);
    const box = screen.getByLabelText("Reply");
    fireEvent.change(box, { target: { value: "postgres" } });
    fireEvent.keyDown(box, { key: "Enter", metaKey: true });
    await waitFor(() => expect(api.calls[0]?.slice(0, 3)).toEqual(["answer", 6, { action: "reply", text: "postgres" }]));
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
