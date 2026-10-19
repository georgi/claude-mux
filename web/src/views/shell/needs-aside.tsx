import { FlagIcon, PanelRightCloseIcon } from "lucide-react";
import { useRef, useState } from "react";
import { type PanelImperativeHandle, useDefaultLayout } from "react-resizable-panels";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { ScrollArea } from "@/components/ui/scroll-area";
import { safeStorage } from "@/lib/storage";

const RAIL = 48;

interface Props {
  count: number;
  main: React.ReactNode;
  children: React.ReactNode;
}

/**
 * The desktop body: the main view and the needs-you column in resizable
 * panels. The column collapses to an icon rail with the count; sizes persist.
 */
export function NeedsLayout({ count, main, children }: Props) {
  const aside = useRef<PanelImperativeHandle | null>(null);
  const [railed, setRailed] = useState(false);
  const layout = useDefaultLayout({ id: "cmux.layout.needs", storage: safeStorage });
  return (
    <ResizablePanelGroup
      id="cmux.layout.needs"
      orientation="horizontal"
      className="min-h-0 flex-1"
      defaultLayout={layout.defaultLayout}
      onLayoutChanged={layout.onLayoutChanged}
    >
      <ResizablePanel id="main" minSize="40" className="flex min-h-0 min-w-0 flex-col">
        <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">{main}</main>
      </ResizablePanel>
      <ResizableHandle withHandle />
      <ResizablePanel
        id="needs"
        panelRef={aside}
        defaultSize={320}
        minSize={240}
        maxSize="50"
        collapsible
        collapsedSize={RAIL}
        onResize={() => setRailed(aside.current?.isCollapsed() ?? false)}
        className="flex min-h-0 flex-col overflow-hidden"
      >
        {railed ? (
          <aside aria-label="Needs you" className="flex h-full flex-col items-center gap-2 py-2">
            <Button
              variant="ghost"
              size="icon"
              className="relative"
              aria-label={`Show needs you${count > 0 ? ` (${count})` : ""}`}
              onClick={() => aside.current?.expand()}
            >
              <FlagIcon />
              {count > 0 && (
                <Badge className="absolute -top-1 -right-1 h-4 min-w-4 px-1 text-[10px] tabular-nums">{count}</Badge>
              )}
            </Button>
          </aside>
        ) : (
          <aside aria-label="Needs you" className="flex h-full min-w-0 flex-col">
            <div className="flex h-10 shrink-0 items-center gap-2 border-b pr-1 pl-3">
              <h2 className="flex grow items-center gap-2 text-sm font-medium">
                <FlagIcon className="size-4 text-muted-foreground" />
                Needs you
                {count > 0 && <Badge>{count}</Badge>}
              </h2>
              <Button variant="ghost" size="icon-sm" aria-label="Collapse needs you" onClick={() => aside.current?.collapse()}>
                <PanelRightCloseIcon />
              </Button>
            </div>
            <ScrollArea className="min-h-0 flex-1">{children}</ScrollArea>
          </aside>
        )}
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}
