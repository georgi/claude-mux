import { ChevronsUpDownIcon, SparklesIcon, XIcon } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { type Api, newKey } from "../../api/http";
import type { RepoInfo } from "../../api/types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from "@/components/ui/command";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Textarea } from "@/components/ui/textarea";
import { FieldError } from "./field-error";
import { MarkdownField } from "./markdown-field";
import { AddRepo, useRepos } from "./repos";
import { errText } from "./state";

interface Picked {
  path: string;
  /** Empty: the server detects the default branch. */
  branch: string;
}

/** A combobox over the known repositories; each pick toggles one. */
function RepoPicker({
  repos,
  picked,
  onToggle,
  disabled,
  invalid,
}: {
  repos: RepoInfo[];
  picked: Picked[];
  onToggle: (path: string) => void;
  disabled?: boolean;
  invalid?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const label = picked.length ? `${picked.length} picked` : "Pick repositories…";
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        render={
          <Button
            type="button"
            variant="outline"
            role="combobox"
            aria-expanded={open}
            aria-invalid={invalid || undefined}
            aria-describedby={invalid ? "plan-repos-error" : undefined}
            aria-label="Repositories"
            disabled={disabled}
            className="w-full justify-between font-normal"
          />
        }
      >
        <span className="truncate">{label}</span>
        <ChevronsUpDownIcon className="opacity-50" />
      </PopoverTrigger>
      <PopoverContent align="start" className="w-(--anchor-width) min-w-72 p-0">
        <Command>
          <CommandInput placeholder="Search repositories" />
          <CommandList>
            <CommandEmpty>No repository matches.</CommandEmpty>
            <CommandGroup>
              {repos.map((r) => {
                const on = picked.some((p) => p.path === r.path);
                return (
                  <CommandItem key={r.path} value={r.path} data-checked={on} onSelect={() => onToggle(r.path)}>
                    <code className="min-w-0 truncate">{r.path}</code>
                    <span className="text-xs text-muted-foreground">{r.base_branch}</span>
                  </CommandItem>
                );
              })}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

interface Errors {
  title?: string;
  repos?: string;
  body?: string;
}

/**
 * New plan (UI design 4.2): a title, repositories with their branch, the plan
 * body, and either a first message for the executor or a Claude terminal
 * that plans it.
 */
export function NewPlan({
  api,
  readOnly,
  onCreated,
  onTerminal,
}: {
  api: Api;
  readOnly?: boolean;
  onCreated: (plan: string) => void;
  onTerminal?: (term: number) => void;
}) {
  const { repos, error: repoError, add } = useRepos(api);
  const [title, setTitle] = useState("");
  const [picked, setPicked] = useState<Picked[]>([]);
  const [body, setBody] = useState("");
  const [request, setRequest] = useState("");
  const [errors, setErrors] = useState<Errors>({});
  const [busy, setBusy] = useState(false);

  const toggle = (path: string) => {
    setPicked((p) => (p.some((x) => x.path === path) ? p.filter((x) => x.path !== path) : [...p, { path, branch: "" }]));
    setErrors((e) => ({ ...e, repos: undefined }));
  };
  const setBranch = (path: string, branch: string) => setPicked((p) => p.map((x) => (x.path === path ? { ...x, branch } : x)));
  const known = (path: string) => repos?.find((r) => r.path === path);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const next: Errors = {};
    if (!title.trim()) next.title = "Give the plan a title.";
    if (!picked.length) next.repos = "Pick at least one repository.";
    setErrors(next);
    if (next.title || next.repos) return;
    setBusy(true);
    try {
      const r = await api.createPlan({
        title: title.trim(),
        repos: picked.map((p) => ({ path: p.path, base_branch: p.branch.trim() || null })),
        ...(request.trim() ? { request: request.trim() } : {}),
      });
      if (body.trim()) {
        try {
          await api.putPlanBody(r.id, { text: body });
        } catch (x) {
          toast.error(`The plan exists, but its body did not save: ${errText(x)}`);
        }
      }
      onCreated(r.id);
    } catch (x) {
      toast.error(errText(x));
    } finally {
      setBusy(false);
    }
  };

  // The TUI's prefix c: a fresh Claude session, here with the text as its first prompt.
  const askClaude = async () => {
    if (!body.trim() && !title.trim()) {
      setErrors({ body: "Write what Claude should plan." });
      return;
    }
    setErrors({});
    setBusy(true);
    try {
      const prompt = title.trim() ? `# ${title.trim()}\n\n${body}`.trim() : body.trim();
      const t = await api.createTerminal({
        kind: "claude",
        args: ["--session-id", newKey(), prompt],
        cwd: picked[0]?.path ?? null,
        rows: null,
        cols: null,
      });
      onTerminal?.(t.id);
    } catch (x) {
      toast.error(errText(x));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="mx-auto w-full max-w-2xl p-4">
      <Card>
        <CardHeader>
          <CardTitle className="text-lg">New plan</CardTitle>
          <CardDescription>Write the plan and its tasks yourself, or ask Claude to plan it.</CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} noValidate className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="plan-title">Title</Label>
              <Input
                id="plan-title"
                value={title}
                aria-invalid={!!errors.title || undefined}
                aria-describedby={errors.title ? "plan-title-error" : undefined}
                onChange={(e) => {
                  setTitle(e.target.value);
                  setErrors((x) => ({ ...x, title: undefined }));
                }}
              />
              <FieldError id="plan-title-error">{errors.title}</FieldError>
            </div>

            <div className="flex flex-col gap-2">
              <span className="text-sm font-medium">Repositories</span>
              {repoError && <p className="text-sm text-destructive">{repoError}</p>}
              {repos === null && !repoError && <p className="text-sm text-muted-foreground">Loading…</p>}
              {repos?.length === 0 && <p className="text-sm text-muted-foreground">The server knows no repositories yet. Add one below.</p>}
              {!!repos?.length && <RepoPicker repos={repos} picked={picked} onToggle={toggle} disabled={readOnly} invalid={!!errors.repos} />}
              <FieldError id="plan-repos-error">{errors.repos}</FieldError>
              {picked.length > 0 && (
                <ul aria-label="Picked repositories" className="flex flex-col gap-2">
                  {picked.map((p, i) => (
                    <li key={p.path} className="flex flex-wrap items-center gap-2 rounded-md border px-2 py-1.5 text-sm">
                      <code className="min-w-0 flex-1 break-all">{p.path}</code>
                      {i === 0 && <Badge variant="secondary">executor runs here</Badge>}
                      <Input
                        aria-label={`Branch for ${p.path}`}
                        className="h-7 w-36"
                        placeholder={known(p.path)?.base_branch ?? "default branch"}
                        value={p.branch}
                        disabled={readOnly}
                        onChange={(e) => setBranch(p.path, e.target.value)}
                      />
                      <Button type="button" variant="ghost" size="icon-sm" aria-label={`Remove ${p.path}`} onClick={() => toggle(p.path)} disabled={readOnly}>
                        <XIcon />
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
              <AddRepo
                disabled={readOnly}
                add={async (s) => {
                  const r = await add(s);
                  setPicked((p) => (p.some((x) => x.path === r.path) ? p : [...p, { path: r.path, branch: "" }]));
                  setErrors((x) => ({ ...x, repos: undefined }));
                  return r;
                }}
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <MarkdownField
                id="plan-body"
                label="Plan body"
                value={body}
                placeholder="Goals, constraints, the shape of the work. Markdown."
                disabled={readOnly}
                onChange={(t) => {
                  setBody(t);
                  setErrors((x) => ({ ...x, body: undefined }));
                }}
              />
              <FieldError id="plan-body-error">{errors.body}</FieldError>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="plan-request">First message for the executor</Label>
              <Textarea id="plan-request" rows={3} value={request} placeholder="Optional" disabled={readOnly} onChange={(e) => setRequest(e.target.value)} />
              <p className="text-xs text-muted-foreground">Leave it empty to add the tasks yourself. Accepting the plan then starts the executor.</p>
            </div>

            <div className="flex flex-wrap gap-2">
              <Button type="submit" disabled={readOnly || busy}>
                {busy ? "Working…" : "Create plan"}
              </Button>
              <Button type="button" variant="outline" disabled={readOnly || busy} onClick={() => void askClaude()}>
                <SparklesIcon /> Ask Claude to plan this
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>
    </section>
  );
}
