// The colour theme. Tiny on purpose: src/main.tsx imports it to set the
// theme before the first paint, so it must not pull in the settings view.

export type Theme = "light" | "dark" | "system";

/** The localStorage key. "light" or "dark"; absent means follow the OS. */
export const THEME_KEY = "cmux.theme";

const listeners = new Set<(t: Theme) => void>();
let osLight: MediaQueryList | null = null;
// The choice when storage is blocked (private mode): it lasts for this page.
let memory: Theme = "system";

export function getTheme(): Theme {
  try {
    const v = localStorage.getItem(THEME_KEY);
    return v === "light" || v === "dark" ? v : "system";
  } catch {
    return memory;
  }
}

/** Dark is the default when the OS has no preference for light. */
function isDark(t: Theme): boolean {
  return t === "system" ? !(osLight?.matches ?? matchMedia("(prefers-color-scheme: light)").matches) : t === "dark";
}

/** Sets `dark` on <html> and the theme-color meta to the active --background. */
export function applyTheme(t: Theme = getTheme()): void {
  const root = document.documentElement;
  root.classList.toggle("dark", isDark(t));
  const bg = getComputedStyle(root).getPropertyValue("--background").trim();
  const meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]');
  if (meta && bg) meta.content = bg;
}

/** Keeps the choice (in this page only when storage is blocked) and applies it. */
export function setTheme(t: Theme): void {
  try {
    if (t === "system") localStorage.removeItem(THEME_KEY);
    else localStorage.setItem(THEME_KEY, t);
  } catch {
    memory = t;
  }
  applyTheme(t);
  for (const f of listeners) f(t);
}

/** Flips light and dark (the palette's "Toggle theme"). */
export function toggleTheme(): void {
  setTheme(document.documentElement.classList.contains("dark") ? "light" : "dark");
}

/** Notifies `f` of every setTheme; returns the unsubscribe. */
export function onTheme(f: (t: Theme) => void): () => void {
  listeners.add(f);
  return () => listeners.delete(f);
}

/** Applies the stored theme now and follows OS changes while it is "system". */
export function initTheme(): void {
  osLight = matchMedia("(prefers-color-scheme: light)");
  applyTheme();
  osLight.addEventListener("change", () => applyTheme());
}
