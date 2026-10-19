import { lazy, Suspense, type ComponentProps } from "react";
import { Skeleton } from "@/components/ui/skeleton";

// xterm and its WebGL addon are large. Load them when a terminal opens.
const TermViewImpl = lazy(() => import("./TermView").then((m) => ({ default: m.TermView })));

export function TermView(props: ComponentProps<typeof TermViewImpl>) {
  return (
    <Suspense fallback={<Skeleton className="m-2 h-[calc(100%-1rem)] min-h-24" aria-busy="true" />}>
      <TermViewImpl {...props} />
    </Suspense>
  );
}

// Streamdown, unified and shiki load with the first reply. Until then the
// raw text shows, so a reply never flashes empty.
const MarkdownImpl = lazy(() => import("@/components/markdown").then((m) => ({ default: m.Markdown })));

export function Markdown({ children, streaming }: { children: string; streaming?: boolean }) {
  return (
    <Suspense fallback={<p className="whitespace-pre-wrap">{children}</p>}>
      <MarkdownImpl streaming={streaming}>{children}</MarkdownImpl>
    </Suspense>
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
const PlanListImpl = lazy(() => import("./Board").then((m) => ({ default: m.PlanList })));
const NewPlanImpl = lazy(() => import("./Board").then((m) => ({ default: m.NewPlan })));
const SettingsImpl = lazy(() => import("./Settings").then((m) => ({ default: m.Settings })));

export function Board(props: ComponentProps<typeof BoardImpl>) {
  return (
    <Suspense fallback={<Loading />}>
      <BoardImpl {...props} />
    </Suspense>
  );
}
export function PlanList(props: ComponentProps<typeof PlanListImpl>) {
  return (
    <Suspense fallback={<Loading />}>
      <PlanListImpl {...props} />
    </Suspense>
  );
}
export function NewPlan(props: ComponentProps<typeof NewPlanImpl>) {
  return (
    <Suspense fallback={<Loading />}>
      <NewPlanImpl {...props} />
    </Suspense>
  );
}
export function Settings(props: ComponentProps<typeof SettingsImpl>) {
  return (
    <Suspense fallback={<Loading />}>
      <SettingsImpl {...props} />
    </Suspense>
  );
}
