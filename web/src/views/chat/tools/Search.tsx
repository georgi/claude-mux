import { FileIcon, FolderIcon, SearchIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Generic } from "./Generic";
import { CLAMP, FullOutputButton, PathChip, relPath, ResultOutput, str, ToolCard, useFullResult, type ToolProps } from "./parts";

export interface SearchResult {
  /** Matches as the tool counts them, else the result lines. */
  count: number;
  /** The result lines, when every one is a file path. */
  paths: string[] | null;
}

const PATH = /^(\/|\.{1,2}\/|[\w@.-]+\/)?[^\s:]+(\/[^\s:]+)*$/;

/**
 * The count and the paths in Grep or Glob output: `Found 3 files` and a path
 * per line, `No files found`, plain paths (Glob), or `path:line:text` lines.
 */
export function parseSearch(out: string): SearchResult {
  let lines = out.split("\n").filter((l) => l.trim() !== "");
  if (lines.length === 0 || /^No (files|matches) found/.test(lines[0])) return { count: 0, paths: null };
  let count: number | undefined;
  const found = /^Found (\d+)/.exec(lines[0]);
  if (found) {
    count = Number(found[1]);
    lines = lines.slice(1);
  }
  // A cut list ends with a note such as "(Results are truncated...)".
  const body = lines.filter((l) => !/^\(.*\)$/.test(l.trim()));
  const paths = body.length > 0 && body.every((l) => PATH.test(l)) ? body : null;
  return { count: count ?? body.length, paths };
}

/** The File Tree pattern, one level deep: each folder with its files under it. */
function FileList({ paths, cwd }: { paths: string[]; cwd?: string }) {
  const dirs = new Map<string, string[]>();
  for (const p of paths) {
    const r = relPath(p, cwd);
    const cut = r.lastIndexOf("/");
    const dir = cut < 0 ? "" : r.slice(0, cut);
    dirs.set(dir, [...(dirs.get(dir) ?? []), r.slice(cut + 1)]);
  }
  return (
    <ul role="tree" aria-label="Matching files" className={`${CLAMP} overflow-auto rounded-lg border p-2 font-mono text-xs`}>
      {[...dirs].map(([dir, files]) => (
        <li key={dir} role="treeitem" aria-expanded="true" aria-selected="false">
          {dir && (
            <span className="flex items-center gap-1.5 text-muted-foreground">
              <FolderIcon className="size-3.5 shrink-0" aria-hidden="true" />
              <span className="truncate">{dir}/</span>
            </span>
          )}
          <ul role="group" className={dir ? "ml-2 border-l pl-3" : undefined}>
            {files.map((f, i) => (
              <li key={i} role="treeitem" aria-selected="false" className="flex items-center gap-1.5 text-foreground">
                <FileIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                <span className="truncate">{f}</span>
              </li>
            ))}
          </ul>
        </li>
      ))}
    </ul>
  );
}

function Results(p: ToolProps) {
  const full = useFullResult(p);
  const { paths } = parseSearch(full.text);
  if (!paths) return <ResultOutput {...p} />;
  return (
    <>
      <FileList paths={paths} cwd={p.cwd} />
      <FullOutputButton full={full} total={p.row.resultLen} />
    </>
  );
}

/** Grep and Glob: the pattern, where, how many hits, and the files as a tree. */
export function Search(p: ToolProps) {
  const pattern = str(p.row.input, "pattern");
  if (!pattern) return <Generic {...p} />;
  const path = str(p.row.input, "path");
  const res = p.row.result !== null && !p.row.isError ? parseSearch(p.row.result) : null;
  const noun = p.row.name === "Glob" || res?.paths ? "file" : "match";
  return (
    <ToolCard
      row={p.row}
      icon={<SearchIcon />}
      open={p.open}
      title={
        <>
          <span className="shrink-0 font-medium">{p.row.name}</span>
          <span className="min-w-0 truncate font-mono text-xs text-foreground">{pattern}</span>
          {path && <PathChip path={path} cwd={p.cwd} className="max-w-[40%]" />}
        </>
      }
      meta={
        res && (
          <Badge variant="secondary" className="h-4 px-1.5 text-[0.65rem]">
            {res.count} {noun === "file" ? (res.count === 1 ? "file" : "files") : res.count === 1 ? "match" : "matches"}
          </Badge>
        )
      }
    >
      {p.row.result ? <Results {...p} /> : null}
    </ToolCard>
  );
}
