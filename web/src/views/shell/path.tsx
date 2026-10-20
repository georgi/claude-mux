import { cn } from "cn";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

/** Splits a path before its last segment, or before its last 16 characters if that segment is long. */
export function splitPath(value: string): [head: string, tail: string] {
  const slash = value.lastIndexOf("/", value.length - 2);
  const at = slash >= 0 && value.length - slash <= 32 ? slash : Math.max(0, value.length - 16);
  return [value.slice(0, at), value.slice(at)];
}

/**
 * A path that truncates in the middle: the start gives way, the last segment
 * stays. The full value is in a tooltip, and the text itself is whole for
 * screen readers.
 */
export function MiddlePath({ value, className }: { value: string; className?: string }) {
  const [head, tail] = splitPath(value);
  return (
    <Tooltip>
      <TooltipTrigger render={<span className={cn("flex min-w-0 max-w-full", className)} />}>
        <span className="truncate">{head}</span>
        <span className="shrink-0 whitespace-pre">{tail}</span>
      </TooltipTrigger>
      <TooltipContent className="max-w-[min(90vw,40rem)] font-mono break-all">{value}</TooltipContent>
    </Tooltip>
  );
}
