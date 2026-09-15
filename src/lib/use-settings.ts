import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getSettings,
  setSettings,
  DEFAULT_FAVORITE_STT_LANGUAGES,
  DEFAULT_REFINEMENT_MODES,
  DEFAULT_QUICK_SWITCH_MODE_IDS,
  type Settings,
  type Provider,
  type RefinementMode,
} from "./settings";
import { loadPersisted, savePersisted } from "./persist";
import { isTauri } from "./runtime";

export async function patchSettings(patch: Partial<Settings>): Promise<void> {
  const base = cached ?? (await init());
  const next: Settings = { ...base, ...patch };
  broadcast(next);
  try {
    await savePersisted(next);
  } catch (e) {
    console.warn("[vibeking] patchSettings save failed:", e);
  }
}

const DEFAULT_SETTINGS: Settings = {
  provider: "deepgram",
  deepgramKey: "",
  groqKey: "",
  openaiKey: "",
  elevenlabsKey: "",
  anthropicKey: "",
  language: "zh",
  sttLanguage: "auto",
  model: null,
  polishProvider: "anthropic",
  polishBaseUrl: "http://localhost:11434/v1",
  polishModel: "qwen2.5:3b-instruct",
  polishApiKey: "",
  polishProviderConfigs: {},
  refinementModes: DEFAULT_REFINEMENT_MODES,
  // Off by default — new users may have no local LLM and no Anthropic key.
  // Refinement is opt-in via Settings. (Backend mirrors this in state.rs.)
  activeRefinementModeId: null,
  quickSwitchModeIds: DEFAULT_QUICK_SWITCH_MODE_IDS,
  soundFx: true,
  hotwords: [],
  translateTarget: "English",
  recordHotkey: "right-control",
  micDevice: "",
  screenContextMode: "ax",
  correctionLearningMode: "ask",
  correctionDictionary: [],
  favoriteSttLanguages: DEFAULT_FAVORITE_STT_LANGUAGES,
  persistentBar: false,
  // Gemma now defaults to the streaming sidecar (graduated from experimental).
  gemmaStreaming: true,
  // Gemma memory: unload the sidecar after 15 min idle (frees ~4-6 GB).
  gemmaKeepLoaded: false,
  gemmaIdleTimeoutMin: 15,
};

// Legacy pre-modes settings shape, used only by `migrateLegacy`. Read out of
// the persisted JSON before it gets coerced into the new `Settings` type.
type LegacySettings = Partial<Settings> & {
  polish?: boolean;
  polishPrompt?: string;
  translatePrompt?: string;
};

const LEGACY_POLISH_PROMPT_DEFAULT =
  "You polish raw voice-typed transcripts. Fix obvious disfluencies, repetition, casing, and punctuation. Preserve the speaker's voice and meaning. Output ONLY the polished text, no preamble.";

const LEGACY_TRANSLATE_PROMPT_DEFAULT =
  "You translate voice-typed transcripts to {target}. Produce a natural, fluent translation. Output ONLY the translated text, no preamble.";

/// One-shot migration from the pre-modes settings shape (polish:bool +
/// polishPrompt + translatePrompt) to the modes list. Runs on every init but
/// is a no-op when the new fields are already present. Returns the patch that
/// should be merged into DEFAULT_SETTINGS.
function migrateLegacy(persisted: LegacySettings): Partial<Settings> {
  const patch: Partial<Settings> = {};

  // Only run if the new fields are missing — once persisted in the new shape,
  // legacy fields stop appearing and the migration silently skips.
  if (!persisted.refinementModes) {
    const modes: RefinementMode[] = DEFAULT_REFINEMENT_MODES.map((m) => ({
      ...m,
    }));

    // Preserve custom Polish prompt if the user had edited it.
    if (
      persisted.polishPrompt &&
      persisted.polishPrompt.trim().length > 0 &&
      persisted.polishPrompt !== LEGACY_POLISH_PROMPT_DEFAULT
    ) {
      const i = modes.findIndex((m) => m.id === "polish");
      if (i >= 0) modes[i] = { ...modes[i], prompt: persisted.polishPrompt };
    }

    if (
      persisted.translatePrompt &&
      persisted.translatePrompt.trim().length > 0 &&
      persisted.translatePrompt !== LEGACY_TRANSLATE_PROMPT_DEFAULT
    ) {
      const i = modes.findIndex((m) => m.id === "translate");
      if (i >= 0)
        modes[i] = { ...modes[i], prompt: persisted.translatePrompt };
    }

    patch.refinementModes = modes;
    patch.quickSwitchModeIds = DEFAULT_QUICK_SWITCH_MODE_IDS;
    // Old polish:false → Off; missing-or-true → Polish active.
    patch.activeRefinementModeId =
      persisted.polish === false ? null : "polish";
  }

  return patch;
}

let cached: Settings | null = null;
const listeners = new Set<(s: Settings) => void>();
let crossWebviewSubscribed = false;

// Subscribe (once per webview) to Rust's `settings:changed` broadcast so
// updates made in one webview (e.g. the chipbar persisting a learned
// correction) propagate live to other webviews (e.g. the Corrections route
// in the main app window). Re-fetches settings from Rust on each tick rather
// than trusting the payload shape — Rust is the source of truth.
async function subscribeCrossWebview(): Promise<void> {
  if (crossWebviewSubscribed || !isTauri()) return;
  crossWebviewSubscribed = true;
  try {
    await listen("settings:changed", () => {
      void getSettings()
        .then((s) => {
          // Skip redundant broadcasts when nothing of interest changed in
          // *this* webview's cache (e.g. the webview that originated the
          // update will already have the new state).
          if (cached && JSON.stringify(cached) === JSON.stringify(s)) return;
          broadcast(s);
        })
        .catch((e) =>
          console.warn("[vibeking] settings:changed refetch failed:", e),
        );
    });
  } catch (e) {
    console.warn("[vibeking] settings:changed subscribe failed:", e);
    crossWebviewSubscribed = false;
  }
}

async function init(): Promise<Settings> {
  if (cached) return cached;
  if (!isTauri()) {
    cached = DEFAULT_SETTINGS;
    return cached;
  }
  void subscribeCrossWebview();
  try {
    const persisted = (await loadPersisted()) as LegacySettings | null;
    if (persisted) {
      // Migration runs unconditionally — it's a no-op once the new fields are
      // present, so the first launch after the upgrade rewrites the legacy
      // shape and subsequent launches skip it.
      const migrated = migrateLegacy(persisted);
      // Strip legacy fields so the next `savePersisted` writes the clean
      // shape — otherwise JS spread carries `polish`, `polishPrompt`, and
      // `translatePrompt` along and they linger in the JSON store forever.
      const {
        polish: _legacyPolish,
        polishPrompt: _legacyPolishPrompt,
        translatePrompt: _legacyTranslatePrompt,
        ...persistedClean
      } = persisted;
      void _legacyPolish;
      void _legacyPolishPrompt;
      void _legacyTranslatePrompt;
      cached = { ...DEFAULT_SETTINGS, ...persistedClean, ...migrated };
      // Gemma 12B was removed — migrate a saved selection to the Gemma 4 (MLX)
      // engine so an old store doesn't push an unknown provider to Rust (which
      // would fail to deserialize the settings).
      if ((cached.provider as string) === "gemma-12b") {
        cached = { ...cached, provider: "gemma" };
      }
      await setSettings(cached);
    } else {
      cached = await getSettings();
    }
  } catch (e) {
    console.warn("[vibeking] settings init failed, using defaults:", e);
    cached = DEFAULT_SETTINGS;
  }
  return cached;
}

function broadcast(s: Settings) {
  cached = s;
  for (const l of listeners) l(s);
}

export function useSettings(): [
  Settings,
  (patch: Partial<Settings>) => Promise<void>,
  boolean,
] {
  const [settings, setLocal] = useState<Settings>(cached ?? DEFAULT_SETTINGS);
  const [ready, setReady] = useState<boolean>(cached !== null);

  useEffect(() => {
    let mounted = true;
    void init().then((s) => {
      if (!mounted) return;
      setLocal(s);
      setReady(true);
    });
    const onChange = (s: Settings) => {
      if (mounted) setLocal(s);
    };
    listeners.add(onChange);
    return () => {
      mounted = false;
      listeners.delete(onChange);
    };
  }, []);

  async function update(patch: Partial<Settings>) {
    // Read from the live module cache, not the component-local closure, so
    // changes made elsewhere (tray menu, other components) don't get clobbered.
    const next: Settings = { ...(cached ?? settings), ...patch };
    broadcast(next);
    try {
      await savePersisted(next);
    } catch (e) {
      console.warn("[vibeking] settings save failed:", e);
    }
  }

  return [settings, update, ready];
}

export const PROVIDERS: Provider[] = [
  "deepgram",
  "groq",
  "openai",
  "elevenlabs",
  "local",
  "fluid-audio",
  "qwen3",
  "gemma",
];

export const PROVIDER_LABELS: Record<Provider, string> = {
  deepgram: "Deepgram Nova-3 (cloud)",
  groq: "Groq Whisper (cloud)",
  openai: "OpenAI gpt-4o (cloud)",
  elevenlabs: "ElevenLabs Scribe v2 (cloud)",
  local: "Local · whisper.cpp + Metal",
  "fluid-audio": "Local · Parakeet + ANE",
  qwen3: "Local · Qwen3-ASR (CJK + multilingual)",
  gemma: "Local · Gemma 4 (MLX, experimental)",
};

// Mirrors Rust's `local_parakeet::ENGINE_ID`. Frontend uses this when
// calling the generalized model commands (download/status/delete) with
// `{ engine: "parakeet" }` — keep both ends in lock-step.
export const PARAKEET_ENGINE_ID = "parakeet-tdt-v3";
