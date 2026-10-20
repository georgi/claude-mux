// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { agent, detail, fakeApi, PLAN, plan, state, task } from "../fixtures";
import { Board } from "./Board";

afterEach(cleanup);
beforeEach(() => localStorage.clear());

const NOW = Date.now();

function boardState() {
  return state({
    plans: [plan({ tasks: ["T-0001", "T-0002", "T-0003"] })],
    tasks: [
      task("T-0001", {
        title: "Schema",
        state: "merged",
        branch: "claude/T-0001",
        pr: { url: "https://github.com/o/r/pull/7", number: 7, ci: "passing", ci_failures: 0 },
        criteria_done: 2,
        criteria_total: 2,
      }),
      task("T-0002", {
        title: "API",
        state: "in_progress",
        deps: ["T-0001"],
        agent: `${PLAN}/T-0002`,
        branch: "claude/T-0002",
        attempt: 2,
        criteria_done: 1,
        criteria_total: 2,
      }),
      task("T-0003", { title: "UI", deps: ["T-0002"] }),
    ],
    agents: [agent("T-0002"), agent("T-0002/lint", { activity: "idle" })],
  });
}

const apiWithDetail = () => {
  const api = fakeApi();
  api.task = async (plan, id) => {
    api.calls.push(["task", plan, id]);
    return detail(boardState().tasks.find((t) => t.id === id)!, {
      body: "Build the **REST** layer.",
      criteria: [
        { text: "cargo test passes", checked: true },
        { text: "docs updated", checked: false },
      ],
      notes: [
        { at: NOW - 3 * 3_600_000, by: `${PLAN}/T-0002`, text: "Started on the handlers." },
        { at: NOW - 5 * 60_000, by: null, text: "Use axum extractors." },
      ],
      instructions: "Prefer small commits.",
    });
  };
  return api;
};

describe("Board", () => {
  it("shows cards by state with branch, PR, attempt, deps and sub-agents", () => {
    render(<Board view={boardState()} plan={PLAN} api={fakeApi()} onOpenAgent={() => {}} />);
    const card = (id: string) => document.querySelector<HTMLElement>(`[data-task="${id}"]`)!;
    expect(card("T-0001").closest("[data-column]")?.getAttribute("data-column")).toBe("merged");
    expect(within(card("T-0001")).getByText("claude/T-0001")).toBeTruthy();
    expect(within(card("T-0001")).getByRole("link").textContent).toContain("#7");
    expect(within(card("T-0001")).getByRole("img", { name: "CI passing" })).toBeTruthy();
    expect(within(card("T-0002")).getByText("attempt 2")).toBeTruthy();
    expect(within(card("T-0002")).getByRole("img", { name: "1 sub-agent" })).toBeTruthy();
    expect(within(card("T-0003")).getByText(/waits on T-0002/)).toBeTruthy();
    expect(screen.getByText("1/3 merged")).toBeTruthy();
  });

  it("opens the task sheet and renders criteria and notes from the TaskDetail", async () => {
    const api = apiWithDetail();
    const opened: string[] = [];
    render(<Board view={boardState()} plan={PLAN} api={api} onOpenAgent={(a) => opened.push(a)} />);
    fireEvent.click(within(document.querySelector<HTMLElement>('[data-task="T-0002"]')!).getByRole("button", { name: /API/ }));

    const sheet = await screen.findByRole("dialog");
    expect(api.calls).toContainEqual(["task", PLAN, "T-0002"]);
    // The criteria checklist.
    const criteria = await within(sheet).findByRole("list", { name: "Criteria list" });
    const items = within(criteria).getAllByRole("listitem");
    expect(items.map((li) => li.textContent)).toEqual(["cargo test passes", "docs updated"]);
    expect(items[0].getAttribute("data-checked")).toBe("true");
    expect(within(sheet).getByText("1 of 2 done")).toBeTruthy();
    // The notes timeline with relative times and authors.
    const notes = within(sheet).getByRole("list", { name: "Notes timeline" });
    expect(within(notes).getByText("Started on the handlers.")).toBeTruthy();
    expect(within(notes).getByText("3h")).toBeTruthy();
    expect(within(notes).getByText("5m")).toBeTruthy();
    expect(within(notes).getByText("T-0002")).toBeTruthy();
    expect(within(notes).getByText("you")).toBeTruthy();
    // Body as markdown and the instructions.
    expect(await within(sheet).findByText("REST")).toBeTruthy();
    expect(within(sheet).getByText("Prefer small commits.")).toBeTruthy();

    // A live agent: no spawn, but blocked and open agent.
    expect(within(sheet).queryByRole("button", { name: /Retry|Spawn/ })).toBeNull();
    fireEvent.click(within(sheet).getByRole("button", { name: /Mark blocked/ }));
    await waitFor(() =>
      expect(api.calls).toContainEqual(["patchTask", PLAN, "T-0002", { title: null, body: null, repo: null, deps: null, state: "blocked" }]),
    );
    fireEvent.click(within(sheet).getByRole("button", { name: /Open agent/ }));
    expect(opened).toEqual([`${PLAN}/T-0002`]);
  });

  it("shows a skeleton until the detail arrives", async () => {
    let resolve: (v: ReturnType<typeof detail>) => void = () => {};
    const api = fakeApi({ task: () => new Promise((r) => (resolve = r)) });
    render(<Board view={boardState()} plan={PLAN} api={api} onOpenAgent={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: /UI/ }));
    const sheet = await screen.findByRole("dialog");
    expect(within(sheet).getByRole("status", { name: "Loading the task" })).toBeTruthy();
    resolve(detail(task("T-0003"), { criteria: [{ text: "renders", checked: false }] }));
    expect(await within(sheet).findByText("renders")).toBeTruthy();
    expect(within(sheet).queryByRole("status", { name: "Loading the task" })).toBeNull();
  });

  it("spawns a todo task with instructions and force", async () => {
    const api = fakeApi();
    render(<Board view={boardState()} plan={PLAN} api={api} onOpenAgent={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: /UI/ }));
    const sheet = await screen.findByRole("dialog");
    fireEvent.change(within(sheet).getByLabelText("Instructions for the agent"), { target: { value: "go slow" } });
    fireEvent.click(within(sheet).getByRole("switch", { name: /Force/ }));
    fireEvent.click(within(sheet).getByRole("button", { name: /^Spawn$/ }));
    await waitFor(() => expect(api.calls).toContainEqual(["spawnTask", PLAN, "T-0003", { instructions: "go slow", force: true }]));
  });

  it("resets the open task's detail and spawn settings when navigating between plans", async () => {
    const a = task("T-0001", { plan: "plan-A", title: "A task" });
    const b = task("T-0001", { plan: "plan-B", title: "B task" });
    const view = state({ plans: [a, b].map((t) => plan({ id: t.plan, tasks: [t.id] })), tasks: [a, b] });
    let resolveB!: (d: ReturnType<typeof detail>) => void;
    const api = fakeApi({ task: async (p) => p === a.plan
      ? detail(a, { body: "A description" })
      : new Promise((resolve) => { resolveB = resolve; }) });
    const board = (p: string) => <Board view={view} plan={p} api={api} onOpenAgent={() => {}} />;
    const { rerender } = render(board(a.plan));
    fireEvent.click(screen.getByRole("button", { name: /A task/ }));
    const sheet = await screen.findByRole("dialog");
    await within(sheet).findByText("A description");
    fireEvent.change(within(sheet).getByLabelText("Instructions for the agent"), { target: { value: "A only" } });
    fireEvent.click(within(sheet).getByRole("switch", { name: /Force/ }));

    rerender(board(b.plan));
    expect(within(sheet).getByRole("status", { name: "Loading the task" })).toBeTruthy();
    expect(within(sheet).queryByText("A description")).toBeNull();
    expect(within(sheet).getByLabelText("Instructions for the agent")).toHaveProperty("value", "");
    expect(within(sheet).getByRole("switch", { name: /Force/ }).getAttribute("aria-checked")).toBe("false");
    await act(async () => resolveB(detail(b, { body: "B description" })));
    expect(within(sheet).getByText("B description")).toBeTruthy();
    fireEvent.click(within(sheet).getByRole("button", { name: "Spawn" }));
    await waitFor(() => expect(api.calls).toContainEqual(["spawnTask", b.plan, b.id, { instructions: null, force: false }]));
  });

  it("ignores a previous task's pending detail after changing the sheet's task", async () => {
    const a = task("T-0001", { title: "First task" });
    const b = task("T-0002", { title: "Second task" });
    const view = state({ plans: [plan({ tasks: [a.id, b.id] })], tasks: [a, b] });
    let resolveA!: (d: ReturnType<typeof detail>) => void;
    const api = fakeApi({ task: async (_, id) => id === a.id
      ? new Promise((resolve) => { resolveA = resolve; })
      : detail(b, { body: "Second description" }) });
    const { TaskSheet } = await import("./board/task-sheet");
    const sheet = (id: string) => <TaskSheet view={view} plan={PLAN} task={id} api={api} onClose={() => {}} onOpenAgent={() => {}} />;
    const { rerender } = render(sheet(a.id));
    await screen.findByRole("status", { name: "Loading the task" });
    rerender(sheet(b.id));
    await screen.findByText("Second description");
    await act(async () => resolveA(detail(a, { body: "First description" })));
    expect(screen.queryByText("First description")).toBeNull();
    expect(screen.getByText("Second description")).toBeTruthy();
  });

  it("switches views and remembers the choice per plan", async () => {
    const { unmount } = render(<Board view={boardState()} plan={PLAN} api={fakeApi()} onOpenAgent={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "List" }));
    expect(await screen.findByRole("list", { name: "Tasks" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Graph" }));
    const graph = await screen.findByRole("group", { name: "Task dependency graph" });
    expect(graph.querySelectorAll("[data-edge]")).toHaveLength(2);
    unmount();

    render(<Board view={boardState()} plan={PLAN} api={apiWithDetail()} onOpenAgent={() => {}} />);
    expect(screen.getByRole("group", { name: "Task dependency graph" })).toBeTruthy();
    // A graph node opens the task.
    fireEvent.click(screen.getByRole("button", { name: "T-0001 Schema, Merged" }));
    expect(await screen.findByRole("dialog")).toBeTruthy();
  });

  it("asks before it cancels the plan", async () => {
    const api = fakeApi();
    render(<Board view={boardState()} plan={PLAN} api={api} onOpenAgent={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: /Cancel plan/ }));
    const dialog = await screen.findByRole("alertdialog");
    expect(api.calls.filter((c) => c[0] === "plan")).toEqual([]);
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel plan" }));
    await waitFor(() => expect(api.calls).toContainEqual(["plan", PLAN, "cancel"]));
  });

  it("shows the plan body in the Plan accordion", async () => {
    const api = fakeApi();
    api.planBody = async (p) => {
      api.calls.push(["planBody", p]);
      return { text: "# Goal\n\nShip the **board**." };
    };
    render(<Board view={boardState()} plan={PLAN} api={api} onOpenAgent={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Plan" }));
    expect(await screen.findByText("board")).toBeTruthy();
    expect(api.calls).toContainEqual(["planBody", PLAN]);
  });
});
