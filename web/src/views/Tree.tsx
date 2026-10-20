import type { StateView } from "../api/types";
import { useSpinner } from "../hooks";
import { buildTree, type Row } from "../tree";

interface Props {
  view: StateView;
  /** The open row's key: an agent id or `term-<id>`. */
  selected: string | null;
  onOpen: (row: Row) => void;
  /** Opens a plan's board from its row. */
  onPlan: (plan: string) => void;
}

export function Tree({ view, selected, onOpen, onPlan }: Props) {
  const spin = useSpinner(view.agents.some((a) => a.activity === "working"));
  const rows = buildTree(view, spin);
  if (rows.length === 0) return <p class="muted pad">No plans or terminals yet.</p>;
  return (
    <ul class="tree" role="tree">
      {rows.map((r) => {
        const isPlan = r.depth === 0 && r.agent === undefined && r.term === undefined;
        const open = isPlan || r.agent !== undefined || r.term !== undefined;
        const go = () => (isPlan ? onPlan(r.key) : onOpen(r));
        const sel = r.key === selected;
        return (
          <li
            key={r.key}
            role="treeitem"
            aria-selected={sel}
            class={`row${r.dim ? " dim" : ""}${open ? " open" : ""}${sel ? " sel" : ""}`}
            style={{ paddingLeft: `${0.5 + r.depth * 1}rem` }}
            tabIndex={open ? 0 : -1}
            onClick={open ? go : undefined}
            onKeyDown={open ? (e) => e.key === "Enter" && go() : undefined}
            title={r.agent ?? (isPlan ? "Open the board" : r.label)}
          >
            <span class="glyph">{r.glyph}</span>
            <span class="label">{r.label}</span>
            {r.detail && <span class="detail">{r.detail}</span>}
          </li>
        );
      })}
    </ul>
  );
}
