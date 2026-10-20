// Highlighted code for the cards that show file content: the language from
// the file's extension, one highlighted node per line.

import { useHighlightedCode } from "@/hooks/use-highlighted-code";

const EXT: Record<string, string> = {
  bash: "bash",
  cjs: "javascript",
  css: "css",
  diff: "diff",
  htm: "html",
  html: "html",
  js: "javascript",
  json: "json",
  jsx: "jsx",
  md: "markdown",
  mjs: "javascript",
  patch: "diff",
  py: "python",
  sh: "bash",
  sql: "sql",
  ts: "typescript",
  tsx: "tsx",
  yaml: "yaml",
  yml: "yaml",
  zsh: "bash",
};

/** The highlighter language of a file, or `text` for none. */
export function langOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
  const dot = name.lastIndexOf(".");
  return (dot > 0 && EXT[name.slice(dot + 1)]) || "text";
}

/** One node per line of `code`, highlighted once the grammar has loaded. */
export function useLines(code: string, lang: string): React.ReactNode[] {
  return useHighlightedCode(code, lang);
}
