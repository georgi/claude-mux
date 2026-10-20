import { KindIcon, ResultOutput, ToolCard, type ToolProps } from "./parts";

/** Any tool: its name, the one-line summary and the raw output. */
export function Generic(p: ToolProps & { icon?: React.ReactNode }) {
  const { row } = p;
  return (
    <ToolCard
      row={row}
      icon={p.icon ?? <KindIcon name={row.name} />}
      open={p.open}
      title={
        <>
          <span className="shrink-0 font-medium">{row.name}</span>
          <span className="min-w-0 truncate font-mono text-xs">{row.summary}</span>
        </>
      }
    >
      {row.result ? <ResultOutput {...p} wrap={row.name !== "Bash"} /> : null}
    </ToolCard>
  );
}
