import { DownloadIcon, MonitorIcon, MoonIcon, SunIcon } from "lucide-react";
import { useEffect, useState } from "react";
import type { Api } from "../../api/http";
import { isIos, isStandalone, useInstall } from "../../pwa";
import { AddRepo, useRepos } from "../Board";
import { Err, SaveState, Section, useSave } from "./common";
import { getTheme, onTheme, setTheme, type Theme } from "./theme";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from "@/components/ui/item";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";

const THEMES: [Theme, string, typeof SunIcon][] = [
  ["light", "Light", SunIcon],
  ["dark", "Dark", MoonIcon],
  ["system", "System", MonitorIcon],
];

export function App() {
  const [theme, setLocal] = useState(getTheme);
  const [save, run] = useSave();
  const install = useInstall();
  useEffect(() => onTheme(setLocal), []);
  const pick = (t: Theme) => void run(async () => setTheme(t));
  const installAction = isStandalone() ? (
    <Badge variant="secondary">Installed</Badge>
  ) : install ? (
    <Button size="sm" onClick={() => void install()}>
      <DownloadIcon /> Install app
    </Button>
  ) : null;
  return (
    <Section id="app" title="App" description="How cmux looks and runs on this device." actions={installAction}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-col gap-0.5">
          <Label id="theme-label">Theme</Label>
          <p className="text-xs text-muted-foreground">System follows the light or dark setting of this device.</p>
        </div>
        <div className="flex items-center gap-2">
          <SaveState save={save} />
          <ToggleGroup
            aria-labelledby="theme-label"
            variant="outline"
            size="sm"
            spacing={0}
            value={[theme]}
            onValueChange={(v: unknown[]) => {
              const t = v[0] as Theme | undefined;
              if (t) pick(t);
            }}
          >
            {THEMES.map(([t, label, Icon]) => (
              <ToggleGroupItem key={t} value={t} aria-label={label}>
                <Icon /> {label}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        </div>
      </div>
      {!isStandalone() && !install && isIos() && (
        <p className="text-sm text-muted-foreground">To install on iPhone or iPad: tap Share → Add to Home Screen.</p>
      )}
    </Section>
  );
}

export function Repositories({ api, readOnly }: { api: Api; readOnly?: boolean }) {
  const { repos, error, add } = useRepos(api);
  const [save, run] = useSave();
  // AddRepo shows its own toast too; the inline state stays by the card title.
  const added = (spec: string) => {
    const p = add(spec);
    void run(() => p);
    return p;
  };
  return (
    <Section id="repositories" title="Repositories" description="Repositories on the server that plans can use." actions={<SaveState save={save} />}>
      <Err text={error} />
      {!repos && !error && <Skeleton className="h-14 w-full" />}
      {repos?.length === 0 && <p className="text-sm text-muted-foreground">No repositories yet. Add one by its path or URL.</p>}
      <ItemGroup className="gap-2">
        {repos?.map((r) => (
          <Item key={r.path} variant="outline" size="sm">
            <ItemContent>
              <ItemTitle className="font-mono text-xs break-all">{r.path}</ItemTitle>
              {r.remote_url && <ItemDescription className="break-all">{r.remote_url}</ItemDescription>}
            </ItemContent>
            <ItemActions>
              <Badge variant="outline">{r.base_branch}</Badge>
            </ItemActions>
          </Item>
        ))}
      </ItemGroup>
      <AddRepo add={added} disabled={readOnly} />
    </Section>
  );
}
