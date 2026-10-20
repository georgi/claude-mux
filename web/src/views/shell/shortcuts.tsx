// The `?` sheet: every binding with keys, grouped, drawn from the same
// registry as the key handler and the palette.

import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Kbd, KbdGroup } from "@/components/ui/kbd";
import { type Binding, BINDINGS, GROUPS, isMac, displayKeys } from "@/keys";

/** One chord as Kbd keys: `⌘ K`, or `g` then `n` for a sequence. */
export function Keys({ keys, mac = isMac() }: { keys: string; mac?: boolean }) {
  return (
    <KbdGroup>
      {displayKeys(keys, mac).map((step, i) => (
        <span key={i} className="inline-flex items-center gap-1">
          {i > 0 && <span className="text-xs text-muted-foreground">then</span>}
          {step.map((k) => (
            <Kbd key={k}>{k}</Kbd>
          ))}
        </span>
      ))}
    </KbdGroup>
  );
}

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Filter for what applies on this device (read-only hides mutating keys). */
  show?: (b: Binding) => boolean;
}

export function ShortcutsSheet({ open, onOpenChange, show = () => true }: Props) {
  const mac = isMac();
  const listed = BINDINGS.filter((b) => b.keys.length > 0 && show(b));
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[80svh] overflow-y-auto sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Keyboard shortcuts</DialogTitle>
          <DialogDescription>Keys work outside text fields and the terminal.</DialogDescription>
        </DialogHeader>
        {GROUPS.map((g) => {
          const rows = listed.filter((b) => b.group === g);
          if (!rows.length) return null;
          return (
            <section key={g} aria-label={g} className="flex flex-col gap-1">
              <h3 className="text-xs font-medium text-muted-foreground">{g}</h3>
              <dl className="flex flex-col">
                {rows.map((b) => (
                  <div key={b.id} className="flex items-center justify-between gap-4 py-1 text-sm">
                    <dt>{b.label}</dt>
                    <dd className="flex items-center gap-2">
                      {b.keys
                        .filter((k, i) => i === 0 || displayKeys(k, mac).join() !== displayKeys(b.keys[0], mac).join())
                        .map((k) => (
                          <Keys key={k} keys={k} mac={mac} />
                        ))}
                    </dd>
                  </div>
                ))}
              </dl>
            </section>
          );
        })}
      </DialogContent>
    </Dialog>
  );
}
