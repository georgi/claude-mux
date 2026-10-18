import { useEffect, useState } from "preact/hooks";
import type { Model, Store } from "./store";

/** The store's model; re-renders on every change. */
export function useModel(store: Store): Model {
  const [m, setM] = useState(store.get());
  useEffect(() => {
    setM(store.get());
    return store.subscribe(() => setM(store.get()));
  }, [store]);
  return m;
}

/** A spinner frame that advances while `on`. */
export function useSpinner(on: boolean): string {
  const frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
  const [i, setI] = useState(0);
  useEffect(() => {
    if (!on) return;
    const t = setInterval(() => setI((x) => (x + 1) % frames.length), 120);
    return () => clearInterval(t);
  }, [on]);
  return frames[i];
}
