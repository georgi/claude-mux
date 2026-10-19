// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { entry, fakeApi, PLAN } from "../fixtures";
import { ChatLog, Composer } from "./Chat";
import { Pairing } from "./Pairing";

afterEach(cleanup);

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
