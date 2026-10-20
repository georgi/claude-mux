// @vitest-environment happy-dom
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { relativeTime, useNow } from "./time";

// Local time, so the weekday and date do not depend on the test machine's zone.
const NOW = new Date(2026, 9, 19, 12, 0, 0).getTime(); // Mon Oct 19 2026, 12:00

describe("relativeTime", () => {
  it("reads 'now' under a minute and for future times", () => {
    expect(relativeTime(NOW, NOW)).toBe("now");
    expect(relativeTime(NOW - 59_000, NOW)).toBe("now");
    expect(relativeTime(NOW + 5 * 60_000, NOW)).toBe("now");
  });

  it("counts minutes, then hours", () => {
    expect(relativeTime(NOW - 60_000, NOW)).toBe("1m");
    expect(relativeTime(NOW - 2 * 60_000 - 30_000, NOW)).toBe("2m");
    expect(relativeTime(NOW - 59 * 60_000, NOW)).toBe("59m");
    expect(relativeTime(NOW - 3 * 3600_000, NOW)).toBe("3h");
    expect(relativeTime(NOW - 23 * 3600_000, NOW)).toBe("23h");
  });

  it("names the weekday within a week", () => {
    expect(relativeTime(new Date(2026, 9, 18, 9).getTime(), NOW)).toBe("Sun");
    expect(relativeTime(new Date(2026, 9, 13, 13).getTime(), NOW)).toBe("Tue");
  });

  it("gives the date after a week, with the year only when it differs", () => {
    expect(relativeTime(new Date(2026, 9, 3, 10).getTime(), NOW)).toBe("Oct 3");
    expect(relativeTime(new Date(2025, 11, 31, 10).getTime(), NOW)).toBe("Dec 31, 2025");
  });
});

describe("useNow", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
  });
  afterEach(() => vi.useRealTimers());

  it("ticks every interval", () => {
    const { result, unmount } = renderHook(() => useNow(1000));
    expect(result.current).toBe(NOW);
    act(() => vi.advanceTimersByTime(1000));
    expect(result.current).toBe(NOW + 1000);
    act(() => vi.advanceTimersByTime(3000));
    expect(result.current).toBe(NOW + 4000);
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
