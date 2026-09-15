import { arch as getArch } from "@tauri-apps/plugin-os";
import { useEffect, useState } from "react";
import { isTauri } from "./runtime";

// Cached so multiple consumers share a single sync read; the plugin-os
// `arch()` value is set at compile-time and never changes at runtime, so
// it's safe to memoize for the lifetime of the webview.
let cachedValue: boolean | null = null;

/**
 * Resolves to true when the host CPU is Apple Silicon (arm64 / aarch64).
 * On non-Tauri contexts (e.g. the Vite dev server in a browser), resolves
 * to `false` — the UI should treat that as "feature unavailable" rather
 * than gambling on a native-only capability.
 *
 * `@tauri-apps/plugin-os`'s `arch()` returns `"aarch64"` on Apple Silicon
 * and `"x86_64"` on Intel Macs. The function is synchronous (the plugin
 * caches a compile-time value in the webview's globals), but we keep this
 * helper async so callers can be agnostic to that — and so future
 * platform-probe additions stay non-breaking.
 */
export async function isAppleSilicon(): Promise<boolean> {
  if (cachedValue !== null) return cachedValue;
  if (!isTauri()) {
    cachedValue = false;
    return cachedValue;
  }
  try {
    const a = getArch();
    cachedValue = a === "aarch64";
    return cachedValue;
  } catch (e) {
    console.warn("[vibeking] isAppleSilicon: arch() failed:", e);
    cachedValue = false;
    return false;
  }
}

/**
 * React hook variant. Returns `null` while the platform check is in flight
 * (caller renders a loading state), then settles to `true`/`false`.
 */
export function useIsAppleSilicon(): boolean | null {
  const [value, setValue] = useState<boolean | null>(cachedValue);

  useEffect(() => {
    if (cachedValue !== null) {
      setValue(cachedValue);
      return;
    }
    let cancelled = false;
    void isAppleSilicon().then((v) => {
      if (!cancelled) setValue(v);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return value;
}
