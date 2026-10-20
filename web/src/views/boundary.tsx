// Error boundaries: one per route (App.tsx) and one per lazy chunk
// (lazy.tsx). A render error offers Retry; a failed chunk load offers Reload,
// because a new deploy renamed the chunk and only a fresh page finds it.

import { RefreshCwIcon, TriangleAlertIcon } from "lucide-react";
import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";

/** True for the error a failed `import()` throws (Chrome, Firefox, Safari). */
export function isChunkError(e: unknown): boolean {
  const msg = e instanceof Error ? `${e.name} ${e.message}` : String(e);
  return /ChunkLoadError|dynamically imported module|Importing a module script failed|error loading dynamically imported module/i.test(
    msg,
  );
}

interface Props {
  children: ReactNode;
  /** A change resets the boundary, so a new route renders again. */
  resetKey?: unknown;
  /** What failed, for the title: "This view", "This part". */
  what?: string;
  /** Shown in place of the error card (a chunk with a plain fallback). */
  fallback?: ReactNode;
}

interface State {
  error: unknown;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { error: error ?? new Error("Unknown error") };
  }

  componentDidUpdate(prev: Props) {
    if (this.state.error && prev.resetKey !== this.props.resetKey) this.setState({ error: null });
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error(error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    if (this.props.fallback !== undefined) return this.props.fallback;
    const chunk = isChunkError(error);
    return (
      <Empty role="alert">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <TriangleAlertIcon className="text-destructive" />
          </EmptyMedia>
          <EmptyTitle>{chunk ? "A new version is available" : `${this.props.what ?? "This view"} failed`}</EmptyTitle>
          <EmptyDescription className="break-words">
            {chunk ? "Part of the app did not load. Reload the page to get it." : error instanceof Error ? error.message : String(error)}
          </EmptyDescription>
        </EmptyHeader>
        <EmptyContent>
          {chunk ? (
            <Button onClick={() => location.reload()}>
              <RefreshCwIcon data-icon="inline-start" />
              Reload
            </Button>
          ) : (
            <Button onClick={() => this.setState({ error: null })}>
              <RefreshCwIcon data-icon="inline-start" />
              Retry
            </Button>
          )}
        </EmptyContent>
      </Empty>
    );
  }
}
