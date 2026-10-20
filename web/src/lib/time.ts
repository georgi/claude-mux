import { useEffect, useState } from "react";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

const weekday = new Intl.DateTimeFormat("en-US", { weekday: "short" });
const monthDay = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric" });
const monthDayYear = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric", year: "numeric" });

/**
 * A short age for a timestamp (both in ms since the epoch): "now" under a
 * minute, then "2m", "3h", the weekday within a week ("Mon"), else the date
 * ("Oct 3", with the year when it differs). Times in the future read "now".
 */
export function relativeTime(ts: number, now: number): string {
  const d = now - ts;
  if (d < MIN) return "now";
  if (d < HOUR) return `${Math.floor(d / MIN)}m`;
  if (d < DAY) return `${Math.floor(d / HOUR)}h`;
  const date = new Date(ts);
  if (d < 7 * DAY) return weekday.format(date);
  return (date.getFullYear() === new Date(now).getFullYear() ? monthDay : monthDayYear).format(date);
}

/** The current time in ms, updated every `intervalMs`. */
export function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}
