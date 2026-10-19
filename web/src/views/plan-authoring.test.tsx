// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { detail, fakeApi, PLAN, plan, state, task } from "../fixtures";
import { Board } from "./Board";
import { byDeps, wouldCycle } from "./board/deps";
import { NewPlan } from "./board/new-plan";
import TaskEditor from "./board/task-editor";

afterEach(cleanup);

const tasks = [task("T-0001", { title: "Schema" }), task("T-0002", { title: "API", deps: ["T-0001"] }), task("T-0003", { title: "UI", deps: ["T-0002"] })];
const draftPlan = plan({ state: "draft", tasks: tasks.map((t) => t.id) });

function editorApi() {
  const api = fakeApi();
  api.task = async (p, id) => {
    api.calls.push(["task", p, id]);
    return detail(tasks.find((t) => t.id === id)!, { body: "body", criteria: [{ text: "works", checked: false }] });
  };
  return api;
}

describe("deps", () => {
  it("finds a cycle through other tasks", () => {
    expect(wouldCycle(tasks, "T-0001", "T-0003")).toBe(true);
    expect(wouldCycle(tasks, "T-0001", "T-0001")).toBe(true);
    expect(wouldCycle(tasks, "T-0003", "T-0001")).toBe(false);
    expect(wouldCycle(tasks, null, "T-0003")).toBe(false);
  });

  it("orders tasks after their deps", () => {
    const out = byDeps([tasks[2], tasks[1], tasks[0]]);
    expect(out.map((t) => t.id)).toEqual(["T-0001", "T-0002", "T-0003"]);
  });
});

describe("TaskEditor", () => {
  it("refuses a dependency that makes a cycle", async () => {
    const api = editorApi();
    render(<TaskEditor plan={draftPlan} tasks={tasks} api={api} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit T-0001" }));
    const form = await screen.findByRole("form", { name: "Edit T-0001" });
    expect(within(form).getByLabelText("Task title")).toHaveProperty("value", "Schema");

    // T-0002 and T-0003 wait on T-0001, so neither can be its dep.
    const deps = within(form).getByRole("group", { name: "Depends on" });
    const api2 = within(deps).getByRole("checkbox", { name: /T-0002/ });
    expect(api2.getAttribute("aria-disabled")).toBe("true");
    expect(deps.textContent).toContain("would make a cycle");
    fireEvent.click(api2);
    expect(within(form).getByRole("alert").textContent).toContain("T-0002 already waits on T-0001");
    expect((api2 as HTMLInputElement).checked).toBe(false);

    fireEvent.click(within(form).getByRole("button", { name: "Save task" }));
    await waitFor(() => expect(api.calls.find((c) => c[0] === "patchTask")).toBeTruthy());
    expect(api.calls.find((c) => c[0] === "patchTask")).toEqual([
      "patchTask",
      PLAN,
      "T-0001",
      { title: "Schema", body: "body", repo: 0, deps: [], state: null },
    ]);
  });

  it("takes a dependency that makes no cycle", async () => {
    const api = editorApi();
    render(<TaskEditor plan={draftPlan} tasks={tasks} api={api} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit T-0003" }));
    const form = await screen.findByRole("form", { name: "Edit T-0003" });
    const box = within(form).getByRole("checkbox", { name: /T-0001/ }) as HTMLInputElement;
    expect(box.getAttribute("aria-disabled")).toBeNull();
    fireEvent.click(box);
    expect(box.checked).toBe(true);
  });

  it("adds and removes criteria, and shows field errors", async () => {
    const api = editorApi();
    render(<TaskEditor plan={draftPlan} tasks={[]} api={api} />);
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    const form = screen.getByRole("form", { name: "New task" });

    // Empty title and no criteria: two field errors, no call.
    fireEvent.click(within(form).getByRole("button", { name: "Add task" }));
    const alerts = within(form).getAllByRole("alert").map((a) => a.textContent);
    expect(alerts).toEqual(["Give the task a title.", "Add at least one criterion."]);
    expect(api.calls.some((c) => c[0] === "createTask")).toBe(false);

    const list = within(form).getByRole("list", { name: "Criteria" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    fireEvent.click(within(form).getByRole("button", { name: "Add criterion" }));
    expect(within(list).getAllByRole("listitem")).toHaveLength(2);
    fireEvent.change(within(form).getByLabelText("Criterion 1"), { target: { value: "first" } });
    fireEvent.change(within(form).getByLabelText("Criterion 2"), { target: { value: "tests pass" } });
    fireEvent.click(within(form).getByRole("button", { name: "Remove criterion 1" }));
    expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    expect(within(form).getByLabelText("Criterion 1")).toHaveProperty("value", "tests pass");

    fireEvent.change(within(form).getByLabelText("Task title"), { target: { value: "Docs" } });
    expect(within(form).queryByText("Give the task a title.")).toBeNull();
    fireEvent.click(within(form).getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(api.calls.find((c) => c[0] === "createTask")).toBeTruthy());
    expect(api.calls.find((c) => c[0] === "createTask")).toEqual([
      "createTask",
      PLAN,
      { title: "Docs", body: "", repo: 0, deps: [], criteria: ["tests pass"] },
    ]);
  });

  it("shows the body preview", async () => {
    render(<TaskEditor plan={draftPlan} tasks={[]} api={editorApi()} />);
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    fireEvent.change(screen.getByLabelText("Task body"), { target: { value: "Add the users table." } });
    fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
    expect(await screen.findByText("Add the users table.")).toBeTruthy();
  });

  it("shows on the board only while the plan is a draft", async () => {
    const api = editorApi();
    const v = (s: "draft" | "accepted") => state({ plans: [plan({ state: s, tasks: tasks.map((t) => t.id) })], tasks });
    const { rerender } = render(<Board view={v("draft")} plan={PLAN} api={api} onOpenAgent={() => {}} />);
    expect(await screen.findByRole("region", { name: "Draft tasks" })).toBeTruthy();
    const order = screen.getByRole("list", { name: "Draft task order" });
    expect(within(order).getAllByRole("listitem").map((li) => li.getAttribute("data-draft-task"))).toEqual(["T-0001", "T-0002", "T-0003"]);
    rerender(<Board view={v("accepted")} plan={PLAN} api={api} onOpenAgent={() => {}} />);
    expect(screen.queryByRole("region", { name: "Draft tasks" })).toBeNull();
  });
});

describe("NewPlan", () => {
  it("shows field errors instead of calling the server", async () => {
    const api = fakeApi();
    render(<NewPlan api={api} onCreated={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Create plan" }));
    const alerts = screen.getAllByRole("alert").map((a) => a.textContent);
    expect(alerts).toEqual(["Give the plan a title.", "Pick at least one repository."]);
    expect(api.calls.some((c) => c[0] === "createPlan")).toBe(false);
  });

  it("creates a plan with its repo, branch and body", async () => {
    const api = fakeApi({
      repos: async () => ({ repos: [{ path: "/srv/a", base_branch: "main", remote_url: null }] }),
    });
    let created = "";
    render(<NewPlan api={api} onCreated={(p) => (created = p)} />);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "auth" } });
    fireEvent.click(await screen.findByRole("combobox", { name: "Repositories" }));
    fireEvent.click(await screen.findByRole("option", { name: /\/srv\/a/ }));
    const picked = screen.getByRole("list", { name: "Picked repositories" });
    fireEvent.change(within(picked).getByLabelText("Branch for /srv/a"), { target: { value: "dev" } });
    fireEvent.change(screen.getByLabelText("Plan body"), { target: { value: "Add **login**." } });
    fireEvent.click(screen.getByRole("button", { name: "Create plan" }));
    await waitFor(() => expect(created).toBe("P-20261018-new"));
    expect(api.calls.find((c) => c[0] === "createPlan")).toEqual(["createPlan", { title: "auth", repos: [{ path: "/srv/a", base_branch: "dev" }] }]);
    expect(api.calls.find((c) => c[0] === "putPlanBody")).toEqual(["putPlanBody", "P-20261018-new", { text: "Add **login**." }]);
  });

  it("asks Claude to plan the text in a new terminal", async () => {
    const api = fakeApi();
    let term = 0;
    render(<NewPlan api={api} onCreated={() => {}} onTerminal={(t) => (term = t)} />);
    fireEvent.click(screen.getByRole("button", { name: /Ask Claude to plan this/ }));
    expect(screen.getByRole("alert").textContent).toBe("Write what Claude should plan.");
    fireEvent.change(screen.getByLabelText("Plan body"), { target: { value: "Split the API." } });
    fireEvent.click(screen.getByRole("button", { name: /Ask Claude to plan this/ }));
    await waitFor(() => expect(term).toBe(7));
    const [, req] = api.calls.find((c) => c[0] === "createTerminal")! as [string, { kind: string; args: string[]; cwd: null }];
    expect(req.kind).toBe("claude");
    expect(req.args[0]).toBe("--session-id");
    expect(req.args[2]).toBe("Split the API.");
    expect(req.cwd).toBeNull();
  });
});
