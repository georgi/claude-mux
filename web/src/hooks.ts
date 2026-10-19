import { useEffect, useState } from "react";
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

/** Does the media query match? Follows changes. */
export function useMedia(query: string): boolean {
  const get = () => typeof matchMedia === "function" && matchMedia(query).matches;
  const [on, setOn] = useState(get);
  useEffect(() => {
    if (typeof matchMedia !== "function") return;
    const mq = matchMedia(query);
    const f = () => setOn(mq.matches);
    f();
    mq.addEventListener("change", f);
    return () => mq.removeEventListener("change", f);
  }, [query]);
  return on;
}
