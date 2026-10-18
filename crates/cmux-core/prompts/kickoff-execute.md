The plan was accepted. Tasks:
{{task_list}}

Each line gives a task's id, its state, its title, its dependencies and its criteria as done/total.

1. Start every task whose dependencies are all merged, using `spawn_task`. Start at most the number of task agents your limit allows. Tasks whose dependencies are not merged wait. A spawn over the limit returns an error, so start the rest when a slot frees.
2. Record each assignment in a note with `add_note`, including the task id and the agent it went to.
3. Then end your turn. Each task agent reports back, and cmux wakes you with a digest of the results.
4. On each wake, ignore item ids you have already handled. Read each new result, check its criteria with evidence, and start any task that became ready.
5. Mark the plan done with `update_plan` only when every task is merged or cancelled.
