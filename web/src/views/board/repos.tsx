import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import type { Api } from "../../api/http";
import type { RepoInfo } from "../../api/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { errText } from "./state";

/**
 * The repositories the server knows, with a way to add one: a `git clone`
 * URL, or a path on the server.
 */
export function useRepos(api: Api): {
  repos: RepoInfo[] | null;
  error: string | null;
  add: (spec: string) => Promise<RepoInfo>;
  /** Loads the list again, e.g. after an error. */
  reload: () => void;
} {
  const [repos, setRepos] = useState<RepoInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [n, setN] = useState(0);
  // biome-ignore lint/correctness/useExhaustiveDependencies: n only triggers a reload.
  useEffect(() => {
    let live = true;
    setError(null);
    api
      .repos()
      .then((r) => live && setRepos(r.repos))
      .catch((e: unknown) => live && setError(errText(e)));
    return () => {
      live = false;
    };
  }, [api, n]);
  const reload = useCallback(() => setN((x) => x + 1), []);
  const add = async (spec: string) => {
    const s = spec.trim();
    const r = await api.addRepo(s.startsWith("/") ? { path: s } : { url: s });
    setRepos((list) => [...(list ?? []).filter((x) => x.path !== r.path), r].sort((a, b) => (a.path < b.path ? -1 : 1)));
    return r;
  };
  return { repos, error, add, reload };
}

export function AddRepo({ add, disabled }: { add: (spec: string) => Promise<RepoInfo>; disabled?: boolean }) {
  const [spec, setSpec] = useState("");
  const [busy, setBusy] = useState(false);
  // Not a <form>: NewPlan renders this inside its own form, and forms must not nest.
  const submit = async () => {
    if (!spec.trim() || busy) return;
    setBusy(true);
    try {
      const r = await add(spec);
      setSpec("");
      toast.success(`Added ${r.path}`);
    } catch (x) {
      toast.error(errText(x));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="flex gap-2">
      <Input
        aria-label="Repository URL or path"
        onKeyDown={(e) => {
          if (e.key !== "Enter") return;
          e.preventDefault();
          void submit();
        }}
        placeholder="git URL to clone, or /path on the server"
        value={spec}
        disabled={disabled}
        onChange={(e) => setSpec(e.target.value)}
      />
      <Button type="button" disabled={disabled || busy || !spec.trim()} onClick={() => void submit()}>
        {busy ? "Adding…" : "Add"}
      </Button>
    </div>
  );
}
