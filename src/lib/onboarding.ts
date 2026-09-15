import { useEffect, useState } from "react";

// Persisted first-run flag. Pattern mirrors `appearance.ts`:
//   - localStorage as source of truth, with in-memory cache to keep reads
//     synchronous for render-time gates.
//   - `storage` event listener so a flip in one webview (e.g. main window)
//     propagates into others (chipbar) without a reload.
//   - React hook returns the live value + a one-shot setter that commits
//     completion. There is no "un-complete" path by design — once done,
//     the user manages provider choice from Settings.
const STORAGE_KEY = "vibeking.onboarding.completed";

const listeners = new Set<(v: boolean) => void>();
let cached: boolean | null = null;

function read(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(STORAGE_KEY) === "1";
  } catch {
    // localStorage blocked (private mode, sandbox) — treat as not completed
    // so onboarding still has a chance to run; worst case is the user sees
    // the card every launch, which is recoverable.
    return false;
  }
}

export function getOnboardingCompleted(): boolean {
  if (cached === null) cached = read();
  return cached;
}

export function markOnboardingCompleted(): void {
  cached = true;
  try {
    window.localStorage.setItem(STORAGE_KEY, "1");
  } catch {
    /* ignore — in-memory cache still reflects completion for this session */
  }
  for (const l of listeners) l(true);
}

// TEMPORARY (dev/testing): clear the completion flag so the onboarding
// scrim re-appears, letting us walk the first-run flow without wiping all
// app storage. Mirrors `markOnboardingCompleted`'s broadcast shape so the
// flip propagates to in-process listeners immediately and to other webviews
// via the `storage` event. Remove together with the temporary Home button.
export function resetOnboarding(): void {
  cached = false;
  try {
    window.localStorage.removeItem(STORAGE_KEY);
  } catch {
    /* ignore — in-memory cache still reflects the reset for this session */
  }
  for (const l of listeners) l(false);
}

let booted = false;
export function bootstrapOnboarding(): void {
  // Subscribe once per webview for cross-webview sync. Read+broadcast
  // shape mirrors `bootstrapAppearance`.
  if (booted) return;
  booted = true;
  if (typeof window === "undefined") return;
  window.addEventListener("storage", (e) => {
    if (e.key !== STORAGE_KEY) return;
    cached = null;
    const next = getOnboardingCompleted();
    for (const l of listeners) l(next);
  });
}

export function useOnboardingCompleted(): [boolean, () => void] {
  const [value, setValue] = useState<boolean>(() => getOnboardingCompleted());

  useEffect(() => {
    bootstrapOnboarding();
    const l = (v: boolean) => setValue(v);
    listeners.add(l);
    return () => {
      listeners.delete(l);
    };
  }, []);

  return [value, markOnboardingCompleted];
}
