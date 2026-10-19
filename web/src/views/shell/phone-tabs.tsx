import { ActivityIcon, FlagIcon, ListChecksIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { cn } from "cn";

export type Tab = "needs" | "plans" | "live" | "settings";

interface Props {
  tab: Tab;
  count: number;
  go: (t: "needs" | "plans" | "live") => void;
}

/** Bottom tab bar for phones (UI design 4.2). */
export function PhoneTabs({ tab, count, go }: Props) {
  const items = [
    { id: "needs", label: "Needs you", Icon: FlagIcon },
    { id: "plans", label: "Plans", Icon: ListChecksIcon },
    { id: "live", label: "Live", Icon: ActivityIcon },
  ] as const;
  return (
    <nav aria-label="Sections" className="shrink-0 border-t bg-background pb-[env(safe-area-inset-bottom)]">
      <ul className="grid grid-cols-3">
        {items.map(({ id, label, Icon }) => (
          <li key={id}>
            <button
              type="button"
              aria-current={tab === id ? "page" : undefined}
              onClick={() => go(id)}
              className={cn(
                "relative flex h-14 w-full flex-col items-center justify-center gap-0.5 text-xs outline-none focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:ring-inset",
                tab === id ? "text-foreground" : "text-muted-foreground",
              )}
            >
              <Icon className="size-5" />
              {label}
              {id === "needs" && count > 0 && (
                <Badge className="absolute top-1.5 left-1/2 ml-2 h-4 min-w-4 px-1 text-[10px]">{count}</Badge>
              )}
            </button>
          </li>
        ))}
      </ul>
    </nav>
  );
}
