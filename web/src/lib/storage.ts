// localStorage that a private window or blocked site data cannot break.

/** The same shape as react-resizable-panels' `LayoutStorage`. */
export const safeStorage = {
  getItem: (k: string): string | null => {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  },
  setItem: (k: string, v: string): void => {
    try {
      localStorage.setItem(k, v);
    } catch {
      // The value lasts for this page only.
    }
  },
};
