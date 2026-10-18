The user wants the following. Draft a plan for it.

{{request}}

1. Read the repository and its existing patterns before you write anything. Keep the plan to what the request needs.
2. If the request spans several independent subsystems, plan only the first coherent slice and say what comes later.
3. If the scope or the acceptance bar is unclear, ask the user one focused question with `ask_user`, and wait for the answer before you write the plan.
4. Write the plan into {{plan_file}}. Cover the goal, the approach, the files each task creates or changes, and what is out of scope.
5. Create each task with `create_task`, giving a title, a body, its dependencies (`deps`) and its acceptance criteria (`criteria`). Keep each task small and testable. Give exact file paths and the command that verifies it. Do not write vague steps such as "handle edge cases" or "add tests".
6. Check the whole plan before you hand it over. Every requirement maps to a task, no dependencies form a cycle, and every task has at least one criterion.
7. Move the plan to `proposed` with `update_plan`. That shows the plan to the user for review.
8. End your turn by telling the user the plan is ready for review. Summarize the tasks in a few lines. Do not start any task until the plan is accepted.
