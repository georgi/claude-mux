// The phone's terminal key bar (UI design 4.2): keys a phone keyboard lacks.
// `Ctrl` is sticky: it applies to the next key typed, then lets go.

export interface Key {
  label: string;
  /** Bytes sent to the PTY; absent for the `Ctrl` modifier. */
  data?: string;
  title: string;
}

export const KEYS: Key[] = [
  { label: "Esc", data: "\x1b", title: "Escape" },
  { label: "Tab", data: "\t", title: "Tab" },
  { label: "Ctrl", title: "Ctrl for the next key" },
  { label: "↑", data: "\x1b[A", title: "Up" },
  { label: "↓", data: "\x1b[B", title: "Down" },
  { label: "←", data: "\x1b[D", title: "Left" },
  { label: "→", data: "\x1b[C", title: "Right" },
  { label: "^C", data: "\x03", title: "Interrupt" },
  { label: "⏎", data: "\r", title: "Enter" },
  { label: "1", data: "1", title: "Option 1" },
  { label: "2", data: "2", title: "Option 2" },
  { label: "3", data: "3", title: "Option 3" },
];

/** `data` with Ctrl held: a letter (or `@[\]^_`) becomes its control code. */
export function withCtrl(data: string): string {
  if (data.length !== 1) return data;
  const c = data.toUpperCase().charCodeAt(0);
  if (c >= 0x40 && c <= 0x5f) return String.fromCharCode(c & 0x1f);
  if (data === " ") return "\x00";
  if (data === "?") return "\x7f";
  return data;
}
