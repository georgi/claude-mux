// The tool card registry: one renderer per tool name, Generic for the rest.
// Chat.tsx loads this module lazily, with the lazy chat chunk; rows without a
// structured input (an older server) never need it.

import { createElement, type ComponentType } from "react";
import { Bash } from "./Bash";
import { Edit } from "./Edit";
import { Generic } from "./Generic";
import { SPAWN_SUBAGENT, type ToolProps } from "./parts";
import { Read } from "./Read";
import { Search } from "./Search";
import { Task } from "./Task";
import { Todo } from "./Todo";
import { Web } from "./Web";
import { Write } from "./Write";

export const RENDERERS: Record<string, ComponentType<ToolProps>> = {
  Bash,
  Edit,
  MultiEdit: Edit,
  Write,
  Read,
  Grep: Search,
  Glob: Search,
  WebFetch: Web,
  WebSearch: Web,
  Task,
  Agent: Task,
  [SPAWN_SUBAGENT]: Task,
  TodoWrite: Todo,
};

/** The card for one tool row. */
export function ToolView(p: ToolProps) {
  return createElement(RENDERERS[p.row.name] ?? Generic, p);
}

export type { ToolContext, ToolProps } from "./parts";
