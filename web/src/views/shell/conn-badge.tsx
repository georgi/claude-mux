import { Badge } from "@/components/ui/badge";
import type { ConnStatus } from "../../store";

/** Connected / reconnecting / offline. `seen` is true once the link has opened. */
export function ConnBadge({ status, seen }: { status: ConnStatus; seen: boolean }) {
  if (status === "open")
    return (
      <Badge variant="secondary" role="status">
        Connected
      </Badge>
    );
  if (status === "connecting")
    return (
      <Badge variant="outline" role="status">
        {seen ? "Reconnecting" : "Connecting"}
      </Badge>
    );
  return (
    <Badge variant="destructive" role="status">
      Offline
    </Badge>
  );
}
