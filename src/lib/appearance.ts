import { useEffect, useState } from "react";

export type Appearance = "light" | "dark" | "system";

const STORAGE_KEY = "vibeking.appearance";
const DEFAULT: Appearance = "system";

const listeners = new Set<(a: Appearance) => void>();
let cached: Appearance | null = null;

function read(): Appearance {
  if (typeof window === "undefined") return DEFAULT;
  try {
    const v = window.localStorage.getItem(STORAGE_KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    /* localStorage blocked — fall through to default */
  }
  return DEFAULT;
}

function prefersDark(): boolean {
  if (typeof window === "undefined") return false;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
}

function resolve(a: Appearance): "light" | "dark" {
  return a === "system" ? (prefersDark() ? "dark" : "light") : a;
}

function paint(a: Appearance) {
  if (typeof document === "undefined") return;
  const resolved = resolve(a);
  const root = document.documentElement;
  root.classList.toggle("dark", resolved === "dark");
  root.style.colorScheme = resolved;
}

export function getAppearance(): Appearance {
  if (cached === null) cached = read();
  return cached;
}

export function setAppearance(next: Appearance): void {
  cached = next;
  try {
    window.localStorage.setItem(STORAGE_KEY, next);
  } catch {
    /* ignore — paint still happens */
  }
  paint(next);
  for (const l of listeners) l(next);
}

let booted = false;
export function bootstrapAppearance(): void {
  if (booted) return;
  booted = true;
  const initial = getAppearance();
  paint(initial);
  if (typeof window !== "undefined" && window.matchMedia) {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      if (getAppearance() === "system") paint("system");
    };
    if (mq.addEventListener) mq.addEventListener("change", onChange);
    else mq.addListener(onChange);
  }
  if (typeof window !== "undefined") {
    window.addEventListener("storage", (e) => {
      if (e.key !== STORAGE_KEY) return;
      cached = null;
      const next = getAppearance();
      paint(next);
      for (const l of listeners) l(next);
    });
  }
}

export function useAppearance(): [Appearance, (a: Appearance) => void] {
  const [value, setValue] = useState<Appearance>(() => getAppearance());
  useEffect(() => {
    const l = (a: Appearance) => setValue(a);
    listeners.add(l);
    return () => {
      listeners.delete(l);
    };
  }, []);
  return [value, setAppearance];
}
