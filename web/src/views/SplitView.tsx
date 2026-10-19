import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";

/** Chat and terminal side by side, in resizable panels. */
export function SplitView({ chat, term }: { chat: React.ReactNode; term: React.ReactNode }) {
  return (
    <ResizablePanelGroup id="cmux.layout.split" orientation="horizontal" className="min-h-0 flex-1">
      <ResizablePanel id="chat" minSize="25" className="flex min-h-0 min-w-0 flex-col">
        {chat}
      </ResizablePanel>
      <ResizableHandle withHandle />
      <ResizablePanel id="terminal" minSize="25" className="flex min-h-0 min-w-0 flex-col">
        {term}
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}
