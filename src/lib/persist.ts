import { load, type Store } from "@tauri-apps/plugin-store";
import type { Settings } from "./settings";
import { setSettings } from "./settings";
import { isTauri } from "./runtime";

const STORE_FILE = "vibeking.settings.json";
const KEY = "settings";

let storePromise: Promise<Store> | null = null;

async function getStore(): Promise<Store | null> {
  if (!isTauri()) return null;
  if (!storePromise) {
    storePromise = load(STORE_FILE, {
      autoSave: true,
      defaults: {},
    });
  }
  return storePromise;
}

export async function loadPersisted(): Promise<Settings | null> {
  try {
    const s = await getStore();
    if (!s) return null;
    const v = await s.get<Settings>(KEY);
    return v ?? null;
  } catch (e) {
    console.warn("[vibeking] loadPersisted failed:", e);
    return null;
  }
}

export async function savePersisted(settings: Settings): Promise<void> {
  if (!isTauri()) return;
  try {
    const s = await getStore();
    if (!s) return;
    await s.set(KEY, settings);
    await s.save();
    await setSettings(settings);
  } catch (e) {
    console.warn("[vibeking] savePersisted failed:", e);
  }
}
