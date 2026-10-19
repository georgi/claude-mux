import { EllipsisIcon, FlagIcon, FolderTreeIcon, ListChecksIcon } from "lucide-react";
import { cn } from "cn";

export type Tab = "needs" | "agents" | "plans" | "more";

interface Props {
  tab: Tab;
  count: number;
  go: (t: Tab) => void;
}

const ITEMS = [
  { id: "needs", label: "Needs you", Icon: FlagIcon },
  { id: "agents", label: "Agents", Icon: FolderTreeIcon },
  { id: "plans", label: "Plans", Icon: ListChecksIcon },
  { id: "more", label: "More", Icon: EllipsisIcon },
] as const;

/** Bottom tab bar for phones (UI design 4.2): thumb reach, 56 px tall, clear of the home indicator. */
export function PhoneTabs({ tab, count, go }: Props) {
  return (
    <nav
      aria-label="Sections"
      className="shrink-0 border-t bg-background pr-[env(safe-area-inset-right)] pb-[env(safe-area-inset-bottom)] pl-[env(safe-area-inset-left)]"
    >
      <ul className="grid grid-cols-4">
        {ITEMS.map(({ id, label, Icon }) => {
          const on = tab === id;
          return (
            <li key={id}>
              <button
                type="button"
                aria-current={on ? "page" : undefined}
                onClick={() => go(id)}
                className={cn(
                  "group/tab flex h-14 min-h-11 w-full min-w-11 flex-col items-center justify-center gap-0.5 text-[0.6875rem] font-medium outline-none select-none [-webkit-tap-highlight-color:transparent] focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:ring-inset",
                  on ? "text-foreground" : "text-muted-foreground active:text-foreground",
                )}
              >
                <span
                  className={cn(
                    "relative flex h-7 w-14 items-center justify-center rounded-full transition-colors duration-(--duration-fast)",
                    on ? "bg-accent text-accent-foreground" : "group-active/tab:bg-accent/60",
                  )}
                >
                  <Icon aria-hidden className="size-5" />
                  {id === "needs" && count > 0 && (
                    <span
                      className="counter absolute -top-1 left-1/2 ml-1.5 h-4 min-w-4 rounded-full bg-status-needs px-1 text-center text-[0.625rem] leading-4 font-semibold text-background"
                    >
                      {count > 99 ? "99+" : count}
                    </span>
                  )}
                </span>
                {/* A space, so the name reads "3 Needs you". */}
                {id === "needs" && count > 0 && " "}
                {label}
              </button>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
