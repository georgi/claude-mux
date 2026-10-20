# cmux web

The browser UI for cmuxd: React, Vite, Tailwind CSS 4 and shadcn/ui on Base UI.

```sh
npm ci
npm run dev     # proxies /v1 to CMUXD_URL (default http://127.0.0.1:7878)
npm run build   # typecheck + production build into dist/
npm test        # vitest
npm run e2e     # playwright
```

## UI building blocks

The chat and agent views follow the shadcn AI Elements patterns (Base UI variant).
Read a pattern's source and usage at `https://ui.shadcn.com/docs/ai-elements/base/<name>.md`.

| Pattern | Purpose |
| --- | --- |
| [Chain of Thought](https://ui.shadcn.com/docs/ai-elements/base/chain-of-thought.md) | Displays a summary that expands into the steps an agent took. |
| [Chat](https://ui.shadcn.com/docs/ai-elements/base/chat.md) | The thread. Message Scroller keeps your place while the reply streams. |
| [Checkpoint](https://ui.shadcn.com/docs/ai-elements/base/checkpoint.md) | Marks a point in the conversation you can return to. |
| [Confirmation](https://ui.shadcn.com/docs/ai-elements/base/confirmation.md) | Asks for approval before a tool runs. |
| [Context](https://ui.shadcn.com/docs/ai-elements/base/context.md) | How much of the context window the conversation has used. |
| [File Tree](https://ui.shadcn.com/docs/ai-elements/base/file-tree.md) | The files the agent changed, as a folder tree. |
| [Inline Citation](https://ui.shadcn.com/docs/ai-elements/base/inline-citation.md) | A source in the sentence that opens on hover. |
| [Message](https://ui.shadcn.com/docs/ai-elements/base/message.md) | One turn. The person's message, and the agent's reply with its actions. |
| [Model Selector](https://ui.shadcn.com/docs/ai-elements/base/model-selector.md) | Two menus. One picks the model the agent runs, the other picks how hard it thinks. |
| [Plan](https://ui.shadcn.com/docs/ai-elements/base/plan.md) | Displays the steps the agent plans to take, and how far it has got. |
| [Prompt Input](https://ui.shadcn.com/docs/ai-elements/base/prompt-input.md) | A message field with attachments and a send button. |
| [Queue](https://ui.shadcn.com/docs/ai-elements/base/queue.md) | Displays the agent's todo list, split into what is left and what is done. |
| [Reasoning](https://ui.shadcn.com/docs/ai-elements/base/reasoning.md) | Displays the agent's thinking as it streams. |
| [Sources](https://ui.shadcn.com/docs/ai-elements/base/sources.md) | Displays the links the agent used for a reply. |
| [Stack Trace](https://ui.shadcn.com/docs/ai-elements/base/stack-trace.md) | Displays a failed tool call and its stack frames. |
| [Subagent](https://ui.shadcn.com/docs/ai-elements/base/subagent.md) | Displays a subagent run, with its tool calls and result. |
| [Suggestion](https://ui.shadcn.com/docs/ai-elements/base/suggestion.md) | A short prompt you can send to the agent in one click. |
| [Task](https://ui.shadcn.com/docs/ai-elements/base/task.md) | Displays the steps the agent takes, as it takes them. |
| [Terminal](https://ui.shadcn.com/docs/ai-elements/base/terminal.md) | Displays a command the agent runs, with its output as it streams. |
| [Test Results](https://ui.shadcn.com/docs/ai-elements/base/test-results.md) | Displays a test run by the agent, with its suites and failures. |
| [Tool](https://ui.shadcn.com/docs/ai-elements/base/tool.md) | Displays a tool call the agent makes, from input to output. |
