import { invoke } from "@tauri-apps/api/core";

export type Provider =
  | "deepgram"
  | "groq"
  | "openai"
  | "elevenlabs"
  | "local"
  | "fluid-audio"
  | "qwen3"
  | "gemma";

/// Every STT provider id, kept right next to the `Provider` type so the two stay
/// in lockstep — add a provider here and to the union together. Use `isProvider`
/// to validate wire/event values (e.g. the tray's `tray:set-provider` payload)
/// instead of a hand-maintained allowlist that silently drifts (which is exactly
/// why a tray switch to qwen3/gemma/fluid-audio failed to reflect in the app).
export const ALL_PROVIDERS: readonly Provider[] = [
  "deepgram",
  "groq",
  "openai",
  "elevenlabs",
  "local",
  "fluid-audio",
  "qwen3",
  "gemma",
];

export function isProvider(value: unknown): value is Provider {
  return (
    typeof value === "string" &&
    (ALL_PROVIDERS as readonly string[]).includes(value)
  );
}

export type PolishProvider = "anthropic" | "ollama" | "lm-studio" | "mlx-lm" | "custom";

/// Remembered base URL + model for one local refinement provider. Persisted
/// per provider so switching providers in Settings restores the user's prior
/// customizations instead of overwriting them with the preset.
export type PolishProviderConfig = { baseUrl: string; model: string };

export type ScreenContextMode = "off" | "ax" | "ocr";

export type CorrectionLearningMode = "off" | "ask" | "auto";

export type CorrectionEntry = {
  from: string;
  to: string;
  learnedAtMs: number;
};

export type RefinementMode = {
  id: string;
  name: string;
  emoji: string;
  prompt: string;
  builtin: boolean;
};

export type Settings = {
  provider: Provider;
  deepgramKey: string;
  groqKey: string;
  openaiKey: string;
  elevenlabsKey: string;
  anthropicKey: string;
  language: string;
  sttLanguage: string;
  model: string | null;
  polishProvider: PolishProvider;
  polishBaseUrl: string;
  polishModel: string;
  polishApiKey: string;
  // Per-provider memory of base URL + model, keyed by provider id. The flat
  // polishBaseUrl/polishModel above are the active config; this preserves each
  // provider's settings across switches. Anthropic has no entry.
  polishProviderConfigs: Partial<Record<PolishProvider, PolishProviderConfig>>;
  // Refinement modes — replaces the old polish:bool + polishPrompt + translatePrompt
  // trio. activeRefinementModeId === null is the "Off" position in the cycle.
  refinementModes: RefinementMode[];
  activeRefinementModeId: string | null;
  quickSwitchModeIds: string[];
  soundFx: boolean;
  hotwords: string[];
  translateTarget: string;
  recordHotkey: string;
  micDevice: string;
  screenContextMode: ScreenContextMode;
  correctionLearningMode: CorrectionLearningMode;
  correctionDictionary: CorrectionEntry[];
  // Languages the user has favorited for the menubar quick-switch.
  // "auto" is implicit — always shown first by the tray, never stored.
  // The tray submenu is favoriteSttLanguages ∩ supportedSttLanguages(provider).
  favoriteSttLanguages: string[];
  // When on, the chip bar stays visible as a compact "idle bar" the user can
  // click to start dictation. The window shrinks to hug the bar while idle.
  persistentBar: boolean;
  // EXPERIMENTAL: route the Gemma live preview through the streaming sidecar
  // (KV-cache reuse) instead of the windowed re-decode. Off by default.
  gemmaStreaming: boolean;
  // Keep the Gemma sidecar resident even when idle (skip the idle-unload).
  // Trades ~4-6 GB RAM for an instant first recording. Only Gemma unloads.
  gemmaKeepLoaded: boolean;
  // Minutes of idle before the Gemma sidecar unloads (when not kept loaded).
  gemmaIdleTimeoutMin: number;
};

type RustRefinementMode = {
  id: string;
  name: string;
  emoji: string;
  prompt: string;
  builtin: boolean;
};

type RustSettings = {
  provider: Provider;
  deepgram_key: string;
  groq_key: string;
  openai_key: string;
  elevenlabs_key: string;
  anthropic_key: string;
  language: string;
  stt_language: string;
  model: string | null;
  polish_provider: PolishProvider;
  polish_base_url: string;
  polish_model: string;
  polish_api_key: string;
  polish_provider_configs: Record<string, { base_url: string; model: string }>;
  refinement_modes: RustRefinementMode[];
  active_refinement_mode_id: string | null;
  quick_switch_mode_ids: string[];
  sound_fx: boolean;
  hotwords: string[];
  translate_target: string;
  record_hotkey: string;
  mic_device: string;
  screen_context_mode: ScreenContextMode;
  correction_learning_mode: CorrectionLearningMode;
  correction_dictionary: { from: string; to: string; learned_at_ms: number }[];
  favorite_stt_languages: string[];
  persistent_bar: boolean;
  gemma_streaming: boolean;
  gemma_keep_loaded: boolean;
  gemma_idle_timeout_min: number;
};

// Curated top-10 menubar quick-switch. Must stay in sync with
// `default_favorite_stt_languages` in src-tauri/src/state.rs — the
// frontend uses this for the empty-array fallback that protects users
// loading from a settings shape that pre-dates the favorites field.
// "auto" is intentionally absent; the tray always prepends it.
export const DEFAULT_FAVORITE_STT_LANGUAGES = [
  "zh", "en", "ja", "ko", "es", "fr", "de", "pt", "ru",
];

// Built-in refinement modes shipped with the app. Must stay in sync with
// `DEFAULT_REFINEMENT_MODES` in src-tauri/src/polish.rs — Rust seeds new
// installs and TS owns the migration from legacy polish_prompt/translate_prompt.
export const DEFAULT_REFINEMENT_MODES: RefinementMode[] = [
  {
    id: "polish",
    name: "Polish",
    emoji: "✨",
    prompt:
      "You polish raw voice-typed transcripts. Fix obvious disfluencies, repetition, casing, and punctuation. Preserve the speaker's voice and meaning. Output ONLY the polished text, no preamble.",
    builtin: true,
  },
  {
    id: "email",
    name: "Email",
    emoji: "✉️",
    prompt:
      "You rewrite raw voice dictation into the body of a business email.\n\nRewrite rules:\n- Tone: professional and warm. Formalize casual fillers (\"wanna\" → \"would like to\", \"gonna\" → \"will\", \"hey\" → \"hi\", drop \"just\", \"basically\", \"like\"). Use complete, clear sentences.\n- Structure: insert blank-line paragraph breaks between distinct ideas. Even a 2-sentence dictation gets a line break between the greeting (if any) and the body. Do NOT produce a single wall of text.\n- Enumerations: when the speaker enumerates items (\"first... second... third...\", \"there are three things: A, B, C\", \"a few points: ...\"), reformat them as a markdown list — one item per line under a short intro sentence ending in a colon. Use numbered items (1. 2. 3.) when the speaker signaled order (\"first/second/third\"); use dash bullets (- ) otherwise.\n- Greeting: KEEP any greeting (\"Hi Benson\") the speaker actually said, verbatim on its own line. Do NOT invent or change the greeting if none was dictated.\n- Sign-off: ALWAYS end with a sign-off paragraph separated by a blank line. If the speaker dictated one (\"Best,\", \"Thanks,\", \"Cheers,\"), preserve it verbatim. Otherwise append exactly this closing on its own two lines: \"Best regards,\" then \"[Your name]\". Always use the literal placeholder \"[Your name]\" when inventing — never guess a sender name.\n- Fidelity: preserve all facts, names, numbers, dates, and the speaker's intent exactly. Never add commitments or content the speaker didn't dictate.\n\nOutput the email body only. No preamble, no surrounding quotes, no explanation.",
    builtin: true,
  },
  {
    id: "code",
    name: "Code prompt",
    emoji: "💻",
    prompt:
      "You rewrite voice-typed dictation as a structured coding prompt suitable for an AI coding assistant. Organize the content into clear sections: Goal, Context, Constraints, and any acceptance criteria the speaker mentioned. Preserve technical terms, identifiers, file names, and library names exactly as spoken. Use code-fence markdown for any literal code or commands. Output ONLY the structured prompt, no preamble.",
    builtin: true,
  },
  {
    id: "notes",
    name: "Notes",
    emoji: "📝",
    prompt:
      "You rewrite voice-typed dictation as well-formatted markdown notes. Add a short title as an H2 header if the speaker introduced a topic. Use bullet points for lists, **bold** for key terms the speaker emphasized, and paragraph breaks for natural shifts in topic. Preserve facts, numbers, and names exactly. Output ONLY the markdown notes, no preamble.",
    builtin: true,
  },
  {
    id: "translate",
    name: "Translate",
    emoji: "🌐",
    prompt:
      "You translate voice-typed transcripts to {target}. Produce a natural, fluent translation. Preserve names, numbers, and technical terms. Output ONLY the translated text, no preamble.",
    builtin: true,
  },
];

export const DEFAULT_QUICK_SWITCH_MODE_IDS = DEFAULT_REFINEMENT_MODES.map(
  (m) => m.id,
);

function toJs(s: RustSettings): Settings {
  return {
    provider: s.provider,
    deepgramKey: s.deepgram_key,
    groqKey: s.groq_key,
    openaiKey: s.openai_key,
    elevenlabsKey: s.elevenlabs_key,
    anthropicKey: s.anthropic_key,
    language: s.language,
    sttLanguage: s.stt_language ?? "auto",
    model: s.model,
    polishProvider: s.polish_provider ?? "anthropic",
    polishBaseUrl: s.polish_base_url ?? POLISH_PRESETS.ollama.baseUrl,
    polishModel: s.polish_model ?? POLISH_PRESETS.ollama.model,
    polishApiKey: s.polish_api_key ?? "",
    polishProviderConfigs: Object.fromEntries(
      Object.entries(s.polish_provider_configs ?? {}).map(([k, v]) => [
        k,
        { baseUrl: v.base_url, model: v.model },
      ]),
    ),
    refinementModes:
      s.refinement_modes && s.refinement_modes.length > 0
        ? s.refinement_modes
        : DEFAULT_REFINEMENT_MODES,
    // `null` is a legitimate value here: the user toggled refinement off.
    // Coalescing to "polish" would clobber that on the next read and the
    // off switch would silently fail to stick.
    activeRefinementModeId: s.active_refinement_mode_id,
    quickSwitchModeIds:
      s.quick_switch_mode_ids && s.quick_switch_mode_ids.length > 0
        ? s.quick_switch_mode_ids
        : DEFAULT_QUICK_SWITCH_MODE_IDS,
    soundFx: s.sound_fx,
    hotwords: s.hotwords,
    translateTarget: s.translate_target,
    recordHotkey: s.record_hotkey ?? "right-control",
    micDevice: s.mic_device ?? "",
    screenContextMode: normalizeScreenContextMode(s.screen_context_mode),
    correctionLearningMode: normalizeCorrectionLearningMode(s.correction_learning_mode),
    correctionDictionary: (s.correction_dictionary ?? []).map((e) => ({
      from: e.from,
      to: e.to,
      learnedAtMs: e.learned_at_ms,
    })),
    // Pre-favorites settings payloads (older builds) don't include the
    // field at all — fall back to the curated set so the tray quick-
    // switch matches what users had before, then they can star/unstar
    // from the Combobox.
    favoriteSttLanguages: s.favorite_stt_languages ?? DEFAULT_FAVORITE_STT_LANGUAGES,
    persistentBar: s.persistent_bar ?? false,
    gemmaStreaming: s.gemma_streaming ?? false,
    gemmaKeepLoaded: s.gemma_keep_loaded ?? false,
    gemmaIdleTimeoutMin: s.gemma_idle_timeout_min ?? 15,
  };
}

function normalizeScreenContextMode(v: unknown): ScreenContextMode {
  return v === "off" || v === "ax" || v === "ocr" ? v : "ax";
}

function normalizeCorrectionLearningMode(v: unknown): CorrectionLearningMode {
  return v === "off" || v === "ask" || v === "auto" ? v : "ask";
}

function toRust(s: Settings): RustSettings {
  return {
    provider: s.provider,
    deepgram_key: s.deepgramKey,
    groq_key: s.groqKey,
    openai_key: s.openaiKey,
    elevenlabs_key: s.elevenlabsKey,
    anthropic_key: s.anthropicKey,
    language: s.language,
    stt_language: s.sttLanguage,
    model: s.model,
    polish_provider: s.polishProvider,
    polish_base_url: s.polishBaseUrl,
    polish_model: s.polishModel,
    polish_api_key: s.polishApiKey,
    polish_provider_configs: Object.fromEntries(
      Object.entries(s.polishProviderConfigs ?? {}).flatMap(([k, v]) =>
        v ? [[k, { base_url: v.baseUrl, model: v.model }]] : [],
      ),
    ),
    refinement_modes: s.refinementModes,
    active_refinement_mode_id: s.activeRefinementModeId,
    quick_switch_mode_ids: s.quickSwitchModeIds,
    sound_fx: s.soundFx,
    hotwords: s.hotwords,
    translate_target: s.translateTarget,
    record_hotkey: s.recordHotkey,
    mic_device: s.micDevice,
    screen_context_mode: s.screenContextMode,
    correction_learning_mode: s.correctionLearningMode,
    correction_dictionary: s.correctionDictionary.map((e) => ({
      from: e.from,
      to: e.to,
      learned_at_ms: e.learnedAtMs,
    })),
    favorite_stt_languages: s.favoriteSttLanguages,
    persistent_bar: s.persistentBar,
    gemma_streaming: s.gemmaStreaming,
    gemma_keep_loaded: s.gemmaKeepLoaded,
    gemma_idle_timeout_min: s.gemmaIdleTimeoutMin,
  };
}

export const RECORD_HOTKEYS: { value: string; label: string }[] = [
  { value: "right-control", label: "⌃ Right Control" },
  { value: "left-control", label: "⌃ Left Control" },
  { value: "right-command", label: "⌘ Right Command" },
  { value: "left-command", label: "⌘ Left Command" },
  { value: "right-option", label: "⌥ Right Option" },
  { value: "left-option", label: "⌥ Left Option" },
  { value: "right-shift", label: "⇧ Right Shift" },
  { value: "left-shift", label: "⇧ Left Shift" },
];

export type PolishPreset = {
  label: string;
  baseUrl: string;
  model: string;
  hint: string;
};

export const POLISH_PRESETS: Record<
  Exclude<PolishProvider, "anthropic">,
  PolishPreset
> = {
  ollama: {
    label: "Ollama",
    baseUrl: "http://localhost:11434/v1",
    model: "qwen2.5:3b-instruct",
    hint: "ollama serve · ollama pull qwen2.5:3b-instruct",
  },
  "lm-studio": {
    label: "LM Studio",
    baseUrl: "http://localhost:1234/v1",
    model: "",
    hint: "LM Studio · Local Server tab · Start server",
  },
  "mlx-lm": {
    label: "mlx-lm",
    baseUrl: "http://localhost:8080/v1",
    model: "mlx-community/Qwen2.5-3B-Instruct-4bit",
    hint: "python -m mlx_lm.server --model …",
  },
  custom: {
    label: "Custom · OpenAI-compatible",
    baseUrl: "http://localhost:8000/v1",
    model: "",
    hint: "Any OpenAI-compatible /v1/chat/completions endpoint",
  },
};

export const POLISH_PROVIDERS: { value: PolishProvider; label: string }[] = [
  { value: "anthropic", label: "Anthropic Claude Haiku (cloud)" },
  { value: "ollama", label: "Ollama (local)" },
  { value: "lm-studio", label: "LM Studio (local)" },
  { value: "mlx-lm", label: "mlx-lm (local)" },
  { value: "custom", label: "Custom · OpenAI-compatible" },
];

export const TRANSLATE_TARGETS: { value: string; label: string }[] = [
  { value: "English", label: "English" },
  { value: "Simplified Chinese", label: "简体中文 · Simplified Chinese" },
  { value: "Traditional Chinese", label: "繁體中文 · Traditional Chinese" },
  { value: "Japanese", label: "日本語 · Japanese" },
  { value: "Korean", label: "한국어 · Korean" },
  { value: "Spanish", label: "Español · Spanish" },
  { value: "French", label: "Français · French" },
  { value: "German", label: "Deutsch · German" },
  { value: "Portuguese", label: "Português · Portuguese" },
];

export async function getSettings(): Promise<Settings> {
  return toJs(await invoke<RustSettings>("get_settings"));
}

export async function listInputDevices(): Promise<string[]> {
  return invoke<string[]>("input_devices");
}

/// Lists models installed on a local OpenAI-compatible refinement server
/// (Ollama / MLX / LM Studio) via its `GET /v1/models` endpoint. Throws when
/// the server is unreachable or errors — callers should catch and fall back to
/// manual model entry.
export async function listPolishModels(
  baseUrl: string,
  apiKey: string,
): Promise<string[]> {
  return invoke<string[]>("list_polish_models", { baseUrl, apiKey });
}

export async function setSettings(settings: Settings): Promise<void> {
  await invoke("set_settings", { settings: toRust(settings) });
}

/// Payload emitted on `refinement:active-changed` by the Rust side whenever the
/// active mode is set/cycled. `mode === null` means the user is in the "Off"
/// position — the chip bar should render the muted "Off" badge.
export type ActiveRefinementMode = { mode: RefinementMode | null };

/// Advance the active mode to the next entry in `quickSwitchModeIds`, wrapping
/// past the last position into "Off" (null). Bound to Shift+Tab while recording.
export async function cycleRefinementMode(): Promise<ActiveRefinementMode> {
  return await invoke<ActiveRefinementMode>("cycle_refinement_mode");
}

/// Set the active refinement mode by id, or pass null to select Off.
export async function setActiveRefinementMode(
  id: string | null,
): Promise<ActiveRefinementMode> {
  return await invoke<ActiveRefinementMode>("set_active_refinement_mode", {
    id,
  });
}

// Per-provider language sets live in `stt-languages.ts` — they need to
// branch on Provider, which is defined in this module. Re-export the
// shared option type here for callers that already import from settings.
export type { SttLanguageOption } from "./stt-languages";
