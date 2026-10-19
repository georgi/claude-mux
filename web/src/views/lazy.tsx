import { lazy, Suspense, type ComponentProps, type ReactNode } from "react";
import { Skeleton } from "@/components/ui/skeleton";
import { ErrorBoundary } from "./boundary";

/** A lazy chunk: a skeleton while it loads, Reload if the load fails. */
function Chunk({ fallback, children, failed }: { fallback: ReactNode; children: ReactNode; failed?: ReactNode }) {
  return (
    <ErrorBoundary what="This part" fallback={failed}>
      <Suspense fallback={fallback}>{children}</Suspense>
    </ErrorBoundary>
  );
}

// xterm and its WebGL addon are large. Load them when a terminal opens.
const TermViewImpl = lazy(() => import("./TermView").then((m) => ({ default: m.TermView })));

export function TermView(props: ComponentProps<typeof TermViewImpl>) {
  return (
    <Chunk fallback={<Skeleton className="m-2 h-[calc(100%-1rem)] min-h-24" aria-busy="true" />}>
      <TermViewImpl {...props} />
    </Chunk>
  );
}

// Streamdown, unified and shiki load with the first reply. Until then the
// raw text shows, so a reply never flashes empty.
const MarkdownImpl = lazy(() => import("@/components/markdown").then((m) => ({ default: m.Markdown })));

export function Markdown({ children, streaming }: { children: string; streaming?: boolean }) {
  return (
    <Chunk fallback={<p className="whitespace-pre-wrap">{children}</p>} failed={<p className="whitespace-pre-wrap">{children}</p>}>
      <MarkdownImpl streaming={streaming}>{children}</MarkdownImpl>
    </Chunk>
  );
}

function Loading() {
  return (
    <div className="flex flex-col gap-2 p-4" aria-busy="true">
      <Skeleton className="h-5 w-1/2" />
      <Skeleton className="h-5 w-2/3" />
      <Skeleton className="h-5 w-1/3" />
    </div>
  );
}

// Board and Settings are routes, not the first screen.
const BoardImpl = lazy(() => import("./Board").then((m) => ({ default: m.Board })));
const PlanListImpl = lazy(() => import("./board/plan-list").then((m) => ({ default: m.PlanList })));
const NewPlanImpl = lazy(() => import("./board/new-plan").then((m) => ({ default: m.NewPlan })));
const SettingsImpl = lazy(() => import("./Settings").then((m) => ({ default: m.Settings })));

export function Board(props: ComponentProps<typeof BoardImpl>) {
  return (
    <Chunk fallback={<Loading />}>
      <BoardImpl {...props} />
    </Chunk>
  );
}
export function PlanList(props: ComponentProps<typeof PlanListImpl>) {
  return (
    <Chunk fallback={<Loading />}>
      <PlanListImpl {...props} />
    </Chunk>
  );
}
export function NewPlan(props: ComponentProps<typeof NewPlanImpl>) {
  return (
    <Chunk fallback={<Loading />}>
      <NewPlanImpl {...props} />
    </Chunk>
  );
}
export function Settings(props: ComponentProps<typeof SettingsImpl>) {
  return (
    <Chunk fallback={<Loading />}>
      <SettingsImpl {...props} />
    </Chunk>
  );
}

// The resizable panels are desktop-only: the needs-you column and Split.
const NeedsLayoutImpl = lazy(() => import("./shell/needs-aside").then((m) => ({ default: m.NeedsLayout })));
const SplitViewImpl = lazy(() => import("./SplitView").then((m) => ({ default: m.SplitView })));

export function NeedsLayout(props: ComponentProps<typeof NeedsLayoutImpl>) {
  return (
    <Chunk fallback={<Loading />}>
      <NeedsLayoutImpl {...props} />
    </Chunk>
  );
}
export function SplitView(props: ComponentProps<typeof SplitViewImpl>) {
  return (
    <Chunk fallback={<Loading />}>
      <SplitViewImpl {...props} />
    </Chunk>
  );
}

// Only an unpaired browser sees it; input-otp stays out of the main chunk.
const PairingImpl = lazy(() => import("./Pairing").then((m) => ({ default: m.Pairing })));

export function Pairing(props: ComponentProps<typeof PairingImpl>) {
  return (
    <Chunk fallback={<Loading />}>
      <PairingImpl {...props} />
    </Chunk>
  );
}
