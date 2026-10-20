// @vitest-environment happy-dom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { AgentView } from "../api/types";
import { Conn } from "../conn";
import { agent, entry, fakeApi, state } from "../fixtures";
import { initial, Store } from "../store";
import { AgentPane } from "./AgentPane";

afterEach(cleanup);

describe("AgentPane activity", () => {
  it.each<AgentView["life"]>(["stopped", { exited: { code: 0 } }, { exited: { code: 1 } }])(
    "ends the live indicators when the agent becomes %j",
    (life) => {
      const running = agent("T-0004", { activity: "working" });
      const store = new Store({
        ...initial,
        chats: { [running.id]: [entry("thinking", { kind: "thinking", text: "Checking the result." })] },
      });
      const props = {
        mode: "chat" as const,
        phone: false,
        conn: new Conn(store, { url: "ws://unused" }),
        store,
        api: fakeApi(),
        onMode() {},
        onOpenAgent() {},
      };
      const { rerender } = render(<AgentPane {...props} view={state({ agents: [running] })} agent={running} />);
      expect(screen.getByRole("status", { name: "Agent activity" }).textContent).toBe("Working…");
      expect(screen.getByRole("button", { name: /Thinking…/ })).toBeTruthy();

      // The daemon retains the last activity when it changes the agent's life.
      const ended = { ...running, life };
      rerender(<AgentPane {...props} view={state({ agents: [ended] })} agent={ended} />);
      expect(screen.queryByRole("status", { name: "Agent activity" })).toBeNull();
      expect(screen.queryByRole("button", { name: /Thinking…/ })).toBeNull();
      expect(screen.getByRole("button", { name: /Thought/ })).toBeTruthy();
    },
  );
});
