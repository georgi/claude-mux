import type { StateView } from "../api/types";
import { useSpinner } from "../hooks";
import { buildTree } from "../tree";

interface Props {
  view: StateView;
  selected: number | null;
  onOpen: (term: number) => void;
}

export function Tree({ view, selected, onOpen }: Props) {
  const spin = useSpinner(view.agents.some((a) => a.activity === "working"));
  const rows = buildTree(view, spin);
  if (rows.length === 0) return <p class="muted pad">No plans or terminals yet.</p>;
  return (
    <ul class="tree" role="tree">
      {rows.map((r) => {
        const term = r.term;
        const open = term !== undefined;
        return (
          <li
            key={r.key}
            role="treeitem"
            aria-selected={open && term === selected}
            class={`row${r.dim ? " dim" : ""}${open ? " open" : ""}${open && term === selected ? " sel" : ""}`}
            style={{ paddingLeft: `${0.5 + r.depth * 1}rem` }}
            tabIndex={open ? 0 : -1}
            onClick={open ? () => onOpen(term) : undefined}
            onKeyDown={open ? (e) => e.key === "Enter" && onOpen(term) : undefined}
            title={r.agent ?? r.label}
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
