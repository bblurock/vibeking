import { useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { Link, useSearchParams } from "react-router-dom";
import {
  ArrowUpRight,
  Loader2,
  Mic,
  Plus,
  ScanText,
  SlidersHorizontal,
  Trash2,
  Wand2,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useSettings } from "@/lib/use-settings";
import { subscribeRecording } from "@/lib/hotkey";
import {
  POLISH_PROVIDERS,
  POLISH_PRESETS,
  TRANSLATE_TARGETS,
  DEFAULT_REFINEMENT_MODES,
  type PolishProvider,
  type ScreenContextMode,
  type CorrectionLearningMode,
  type RefinementMode,
  type Settings,
} from "@/lib/settings";
import {
  supportedSttLanguages,
  sttLanguageSupported,
} from "@/lib/stt-languages";
import { isTauri } from "@/lib/runtime";
import { useAppearance, type Appearance } from "@/lib/appearance";
import { Switch } from "@/components/ui/switch";
import { Select } from "@/components/ui/select";
import { Combobox } from "@/components/ui/combobox";
import { Kbd } from "@/components/ui/kbd";
import { InputKey } from "@/components/ui/input-key";
import { InputText } from "@/components/ui/input-text";
import { ModelCombobox } from "@/components/ui/model-combobox";
import { Tabs, type TabItem } from "@/components/ui/tabs";
import { PromptEditor } from "@/components/ui/prompt-editor";
import { HotkeyRecorder } from "@/components/ui/hotkey-recorder";
import { Section, SettingsCard } from "@/components/ui/section";
import {
  SettingsBento,
  BentoRow,
  BentoBlock,
} from "@/components/SettingsBento";
import { StatusPill, TranslateBadge, type Status } from "@/components/StatusPill";
import { EnginePicker } from "@/components/EnginePicker";
import { listInputDevices, listPolishModels } from "@/lib/settings";
import { getStats } from "@/lib/history";
import { setSoundFx } from "@/lib/sound";
import { BrandChip } from "@/components/BrandMark";
import {
  subscribeContextCaptured,
  type ContextCapturedEvent,
} from "@/lib/context-events";
// TEMPORARY (dev/testing): paired with the "Replay onboarding" button below.
import { resetOnboarding } from "@/lib/onboarding";

export function Home() {
  const { t, i18n } = useTranslation();
  const [settings, update, ready] = useSettings();
  const [appearance, setAppearance] = useAppearance();
  const [status, setStatus] = useState<Status>("ready");

  // Translation is now just one of the refinement modes. The TranslateBadge
  // in the header lights up whenever the active mode is `translate`, matching
  // the pre-modes behavior without a separate runtime flag.
  const translateOn = settings.activeRefinementModeId === "translate";

  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void subscribeRecording((event) => {
      if (event.type === "start") setStatus("recording");
      else setStatus("ready");
    }).then((off) => (cleanup = off));
    return () => cleanup?.();
  }, []);

  // Repair stored sttLanguage when it no longer fits the active provider.
  // Happens on first load after the per-provider lists were introduced,
  // or any time something writes an unsupported code to settings
  // (e.g. the tray menu can still set `zh` while Parakeet is active).
  // Without this the dropdown's trigger renders the empty placeholder.
  useEffect(() => {
    if (!ready) return;
    if (!sttLanguageSupported(settings.provider, settings.sttLanguage)) {
      void update({ sttLanguage: "auto" });
    }
  }, [ready, settings.provider, settings.sttLanguage, update]);

  // Settings are grouped into tabs to tame the long single-column scroll. The
  // active tab lives in the URL (`?tab=`) so it survives reloads and is
  // deep-linkable; other params (e.g. ?debug=1) are preserved. NOTE: this hook
  // must run before the early `!ready` return below — hooks can't be skipped.
  const [searchParams, setSearchParams] = useSearchParams();
  const tab = searchParams.get("tab") ?? "dictation";
  const setTab = (id: string) =>
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.set("tab", id);
        return next;
      },
      { replace: true },
    );
  const tabItems: TabItem[] = [
    { id: "dictation", label: t("home.tabDictation"), icon: Mic },
    { id: "refinement", label: t("home.tabRefinement"), icon: Wand2 },
    { id: "context", label: t("home.tabContext"), icon: ScanText },
    { id: "general", label: t("home.tabGeneral"), icon: SlidersHorizontal },
  ];

  if (!ready) {
    return (
      <div className="flex h-full items-center justify-center text-[12.5px] text-[var(--vk-text-8)]">
        <Loader2 className="size-4 animate-spin mr-2" />
        Loading
      </div>
    );
  }

  const statusLabel =
    status === "ready"
      ? t("app.ready")
      : status === "recording"
        ? t("app.recording")
        : t("app.recognizing");

  return (
    <div className="px-10 py-10 max-w-[680px] mx-auto animate-vibeking-fade-up">
      <Header
        status={status}
        statusLabel={statusLabel}
        translateOn={translateOn}
        translateTarget={settings.translateTarget}
      />
      <StatsStrip hotwordCount={settings.hotwords.length} />

      <div className="mt-6">
        <Tabs
          items={tabItems}
          value={tab}
          onChange={setTab}
          aria-label={t("home.settingsTabsLabel")}
        />
      </div>

      {/* Each bento owns one concern. The toggle sits in the header so the
          user sees the switch and what it controls in the same glance.
          When a bento's toggle is off, the body grays to 50% + becomes
          inert (no pointer events, skipped by AT). Visual hierarchy is the
          bento, not the eyebrow — eyebrows are dropped in favor of clear,
          spoken-language titles. Bentos are grouped into the tabs above. */}

      <div className="mt-4 space-y-4">
        {tab === "dictation" && (
        <>
        {/* Recording — always on. No header control. */}
        <SettingsBento
          title={t("home.recordingTitle")}
          description={t("home.recordingDescription")}
        >
          <BentoRow
            label={t("home.recordHotkey")}
            hint={t("home.recordHotkeyHint")}
          >
            <HotkeyRecorder
              value={settings.recordHotkey}
              onChange={(v) => void update({ recordHotkey: v })}
            />
          </BentoRow>
          <BentoRow label={t("home.microphone")}>
            <MicrophoneSelect
              value={settings.micDevice}
              onChange={(v) => void update({ micDevice: v })}
              systemLabel={t("home.microphoneSystem")}
            />
          </BentoRow>
          <BentoRow label={t("home.cancelRecord")}>
            <Kbd>{t("home.cancelRecordEscBadge")}</Kbd>
          </BentoRow>
          <BentoRow
            label={t("home.persistentBar")}
            hint={t("home.persistentBarHint")}
          >
            <Switch
              checked={settings.persistentBar}
              onChange={(on) => void update({ persistentBar: on })}
              aria-label={t("home.persistentBar")}
            />
          </BentoRow>
        </SettingsBento>

        {/* Speech-to-text — always on. Provider + language + hotwords. */}
        <SettingsBento
          title={t("home.sttTitle")}
          description={t("home.sttDescription")}
        >
          <EnginePicker settings={settings} update={update} />
          <BentoRow
            label={t("home.sttLanguage")}
            hint={sttLanguageHint(t, settings)}
          >
            <Combobox
              value={settings.sttLanguage}
              onChange={(v) => void update({ sttLanguage: v })}
              options={supportedSttLanguages(
                settings.provider,
                t("home.sttLanguageAuto"),
              )}
              searchPlaceholder={t("home.sttLanguageSearchPlaceholder")}
              emptyLabel={t("home.sttLanguageNoMatch")}
              isFavorite={(v) => settings.favoriteSttLanguages.includes(v)}
              // "auto" is always present in the tray and isn't a real
              // language — keep the star off it so users can't unstar
              // the implicit sentinel.
              isFavoritable={(v) => v !== "auto"}
              onToggleFavorite={(v) => {
                const current = settings.favoriteSttLanguages;
                const next = current.includes(v)
                  ? current.filter((x) => x !== v)
                  : [...current, v];
                void update({ favoriteSttLanguages: next });
              }}
              favoriteAddLabel={t("home.sttLanguageFavoriteAdd")}
              favoriteRemoveLabel={t("home.sttLanguageFavoriteRemove")}
            />
          </BentoRow>
          <BentoRow
            label={t("home.bentoHotwords")}
            hint={
              settings.hotwords.length === 0
                ? t("home.bentoHotwordsHint")
                : t("home.countActive", { n: settings.hotwords.length })
            }
          >
            <Link
              to="/hotwords"
              className="inline-flex items-center gap-1 h-7 px-2.5 rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-surface)] text-[12px] font-medium text-[var(--vk-text-4)] hover:bg-[var(--vk-canvas-3)] hover:border-[var(--vk-border-strong-2)] transition-colors"
            >
              {t("common.edit")}
              <ArrowUpRight className="size-3" />
            </Link>
          </BentoRow>
        </SettingsBento>
        </>
        )}

        {tab === "refinement" && (
        <>
        <RefinementEngineBento settings={settings} update={update} />
        <RefinementModesBento settings={settings} update={update} />
        </>
        )}

        {tab === "context" && (
        <>
        {/* Context — 3-mode select in header (Off / AX / OCR). */}
        <SettingsBento
          title={t("home.screenContextTitle")}
          description={
            settings.screenContextMode === "ax"
              ? t("home.screenContextAxHint")
              : settings.screenContextMode === "ocr"
                ? t("home.screenContextOcrHint")
                : t("home.screenContextAxFallback")
          }
          disabled={settings.screenContextMode === "off"}
          disabledNote={t("home.translationOff")}
          controlLabel={t("home.screenContextSourceLabel")}
          control={
            <Select
              value={settings.screenContextMode}
              onChange={(v) =>
                void update({ screenContextMode: v as ScreenContextMode })
              }
              options={[
                { value: "off", label: t("home.screenContextOff") },
                { value: "ax", label: t("home.screenContextAx") },
                { value: "ocr", label: t("home.screenContextOcr") },
              ]}
            />
          }
        >
          <BentoRow
            label={
              settings.screenContextMode === "ax"
                ? t("home.screenContextAxHint")
                : t("home.screenContextOcrHint")
            }
            hint={
              settings.screenContextMode === "ax"
                ? t("home.screenContextAxFallback")
                : ""
            }
          />
        </SettingsBento>

        {/* Correction learning — 3-way Off/Ask/Auto. */}
        <SettingsBento
          title={t("home.correctionLearningTitle")}
          description={
            settings.correctionLearningMode === "auto"
              ? t("home.correctionLearningAutoDescription")
              : settings.correctionLearningMode === "ask"
                ? t("home.correctionLearningAskDescription")
                : t("home.correctionLearningOffDescription")
          }
          disabled={settings.correctionLearningMode === "off"}
          disabledNote={t("home.translationOff")}
          controlLabel={t("home.correctionLearningModeLabel")}
          control={
            <Select
              value={settings.correctionLearningMode}
              onChange={(v) =>
                void update({
                  correctionLearningMode: v as CorrectionLearningMode,
                })
              }
              options={[
                { value: "off", label: t("home.correctionLearningOff") },
                { value: "ask", label: t("home.correctionLearningAsk") },
                { value: "auto", label: t("home.correctionLearningAuto") },
              ]}
            />
          }
        >
          <BentoRow
            label={t("home.correctionLearningDictLabel")}
            hint={
              settings.correctionDictionary.length === 0
                ? t("home.correctionLearningEmptyHint")
                : t("home.countActive", {
                    n: settings.correctionDictionary.length,
                  })
            }
          >
            <Link
              to="/corrections"
              className="inline-flex items-center gap-1 h-7 px-2.5 rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-surface)] text-[12px] font-medium text-[var(--vk-text-4)] hover:bg-[var(--vk-canvas-3)] hover:border-[var(--vk-border-strong-2)] transition-colors"
            >
              {t("common.edit")}
              <ArrowUpRight className="size-3" />
            </Link>
          </BentoRow>
        </SettingsBento>
        </>
        )}

        {tab === "general" && (
        <>
        {/* General — always-on app preferences. */}
        <SettingsBento
          title={t("home.generalTitle")}
          description={t("home.generalDescription")}
        >
          <BentoRow label={t("home.language")}>
            <Select
              // Backward compat: users on the old build stored "zh"
              // which used to mean Simplified Chinese — map it to zh-CN so
              // the picker doesn't render as undefined-selected.
              value={
                settings.language === "zh" ? "zh-CN" : settings.language
              }
              onChange={(v) => {
                void update({ language: v });
                void i18n.changeLanguage(v);
              }}
              options={[
                { value: "zh-CN", label: "简体中文" },
                { value: "zh-TW", label: "繁體中文" },
                { value: "en", label: "English" },
              ]}
            />
          </BentoRow>
          <BentoRow label={t("home.appearance")}>
            <Select
              value={appearance}
              onChange={(v) => setAppearance(v as Appearance)}
              options={[
                { value: "light", label: t("home.appearanceLight") },
                { value: "dark", label: t("home.appearanceDark") },
                { value: "system", label: t("home.appearanceSystem") },
              ]}
            />
          </BentoRow>
          <BentoRow
            label={t("home.soundFx")}
            hint={t("home.soundFxHint")}
          >
            <Switch
              checked={settings.soundFx}
              onChange={(on) => {
                setSoundFx(on);
                void update({ soundFx: on });
              }}
              aria-label={t("home.soundFx")}
            />
          </BentoRow>
          {/* TEMPORARY (dev/testing): replay the first-run onboarding flow.
              Clears the localStorage completion flag so PermissionsGate
              re-shows the onboarding scrim. Remove this row together with
              `resetOnboarding` in lib/onboarding.ts before shipping. */}
          <BentoRow
            label="Replay onboarding"
            hint="Temporary — re-shows the first-run flow"
          >
            <button
              type="button"
              onClick={() => {
                resetOnboarding();
                // Reload so PermissionsGate re-reads the (now-cleared) flag
                // from localStorage and re-mounts the onboarding scrim. This
                // avoids depending on the in-memory listener broadcast, which
                // can be a stale no-op after a Vite HMR module swap.
                window.location.reload();
              }}
              className="inline-flex items-center h-7 px-2.5 rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-surface)] text-[12px] font-medium text-[var(--vk-text-4)] hover:bg-[var(--vk-canvas-3)] hover:border-[var(--vk-border-strong-2)] transition-colors"
            >
              Replay
            </button>
          </BentoRow>
        </SettingsBento>

        </>
        )}
      </div>

      <DebugContextPanel />
    </div>
  );
}

// Shared LLM provider config — every refinement mode rewrites through it.
// Lives in its own bento so engine setup is visibly distinct from per-mode
// prompt editing happening below.
function RefinementEngineBento({
  settings,
  update,
}: {
  settings: Settings;
  update: (patch: Partial<Settings>) => Promise<void>;
}) {
  const { t } = useTranslation();

  // Installed-model discovery for the active local provider. Keyed by base URL
  // so switching providers (which rewrites polishBaseUrl) re-fetches. A failed
  // fetch (server not running) leaves `models` empty and surfaces via `error`
  // — the combobox still accepts free-text, so refinement stays usable.
  const isLocalProvider = settings.polishProvider !== "anthropic";
  const baseUrl = settings.polishBaseUrl;
  const apiKey = settings.polishApiKey;
  const [models, setModels] = useState<string[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [refreshNonce, setRefreshNonce] = useState(0);

  useEffect(() => {
    if (!isLocalProvider || !baseUrl) {
      setModels([]);
      setModelsError(null);
      return;
    }
    let cancelled = false;
    setModelsLoading(true);
    setModelsError(null);
    // Debounce so typing into the base-URL field doesn't fire a request per keystroke.
    const timer = setTimeout(() => {
      listPolishModels(baseUrl, apiKey)
        .then((list) => {
          if (cancelled) return;
          setModels(list);
          setModelsError(null);
        })
        .catch((e) => {
          if (cancelled) return;
          setModels([]);
          setModelsError(String(e));
        })
        .finally(() => {
          if (!cancelled) setModelsLoading(false);
        });
    }, 400);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [isLocalProvider, baseUrl, apiKey, refreshNonce]);

  // Persist an edit to the active provider's base URL / model into both the
  // flat active fields (what the backend uses) and the per-provider memory map
  // — so switching providers away and back restores these instead of the preset.
  const saveActiveConfig = (next: { baseUrl?: string; model?: string }) => {
    const provider = settings.polishProvider;
    const baseUrl = next.baseUrl ?? settings.polishBaseUrl;
    const model = next.model ?? settings.polishModel;
    void update({
      polishBaseUrl: baseUrl,
      polishModel: model,
      polishProviderConfigs: {
        ...settings.polishProviderConfigs,
        [provider]: { baseUrl, model },
      },
    });
  };

  return (
    <SettingsBento
      title={t("home.refinementEngineTitle")}
      description={t("home.refinementEngineDescription")}
    >
      <BentoRow label={t("home.bentoLlmProvider")}>
        <Select
          value={settings.polishProvider}
          onChange={(v) => {
            const next = v as PolishProvider;
            const patch: Partial<Settings> = { polishProvider: next };
            if (next !== "anthropic") {
              // Restore what the user previously configured for this provider,
              // falling back to the preset only when they've never touched it.
              const remembered = settings.polishProviderConfigs[next];
              const preset = POLISH_PRESETS[next];
              patch.polishBaseUrl = remembered?.baseUrl ?? preset?.baseUrl ?? "";
              patch.polishModel = remembered?.model ?? preset?.model ?? "";
            }
            void update(patch);
          }}
          options={POLISH_PROVIDERS}
        />
      </BentoRow>
      {settings.polishProvider === "anthropic" ? (
        <BentoRow label={t("home.bentoApiKey")}>
          <InputKey
            value={settings.anthropicKey}
            onChange={(v) => void update({ anthropicKey: v })}
          />
        </BentoRow>
      ) : (
        <>
          <BentoRow
            label={t("home.bentoBaseUrl")}
            hint={POLISH_PRESETS[settings.polishProvider]?.hint ?? ""}
          >
            <InputText
              value={settings.polishBaseUrl}
              onChange={(v) => saveActiveConfig({ baseUrl: v })}
              placeholder="http://localhost:11434/v1"
            />
          </BentoRow>
          <BentoRow
            label={t("home.bentoModel")}
            hint={
              settings.polishProvider === "lm-studio"
                ? t("home.modelHintAnthropic")
                : settings.polishProvider === "mlx-lm"
                  ? t("home.modelHintMlx")
                  : settings.polishProvider === "custom"
                    ? t("home.modelHintLmStudio")
                    : t("home.modelHintOllama")
            }
          >
            <ModelCombobox
              value={settings.polishModel}
              onChange={(v) => saveActiveConfig({ model: v })}
              models={models}
              loading={modelsLoading}
              error={modelsError}
              onRefresh={() => setRefreshNonce((n) => n + 1)}
              placeholder="qwen2.5:3b-instruct"
            />
          </BentoRow>
          <BentoRow
            label={t("home.bentoApiKey")}
            hint={t("home.apiKeyOptionalHint")}
          >
            <InputKey
              value={settings.polishApiKey}
              onChange={(v) => void update({ polishApiKey: v })}
            />
          </BentoRow>
        </>
      )}
    </SettingsBento>
  );
}

// Refinement modes — sidebar list + selected-mode editor.
//
//  - Header switch toggles refinement on/off globally. When off, the body
//    grays out via SettingsBento's built-in disabled treatment.
//  - Sidebar lists every mode. The selected row IS the active mode — one
//    tap = pick AND make active. The header switch's "off" state is the
//    only way to truly disable refinement.
//  - Editor for the selected mode owns mode-specific config (Translate's
//    target language), the prompt, and the per-mode quick-switch toggle.
function RefinementModesBento({
  settings,
  update,
}: {
  settings: Settings;
  update: (patch: Partial<Settings>) => Promise<void>;
}) {
  const { t } = useTranslation();
  const enabled = settings.activeRefinementModeId !== null;

  // Sidebar selection mirrors the active mode when refinement is on. When
  // refinement is off, we still need a "preferred" mode so flipping the
  // switch on has somewhere to land — fall back to the first mode.
  const fallbackId =
    settings.activeRefinementModeId ??
    settings.refinementModes[0]?.id ??
    null;
  const [pendingId, setPendingId] = useState<string | null>(fallbackId);
  const selectedId = settings.activeRefinementModeId ?? pendingId;
  const selectedMode =
    settings.refinementModes.find((m) => m.id === selectedId) ??
    settings.refinementModes[0] ??
    null;

  function selectMode(id: string) {
    setPendingId(id);
    void update({ activeRefinementModeId: id });
  }

  function toggleEnabled(checked: boolean) {
    if (checked) {
      const next = selectedMode?.id ?? settings.refinementModes[0]?.id ?? null;
      void update({ activeRefinementModeId: next });
    } else {
      // Remember the current selection so flipping the switch back on
      // restores the user's last-used mode, not just the first available.
      setPendingId(settings.activeRefinementModeId ?? pendingId);
      void update({ activeRefinementModeId: null });
    }
  }

  function updateMode(id: string, patch: Partial<RefinementMode>) {
    const next = settings.refinementModes.map((m) =>
      m.id === id ? { ...m, ...patch } : m,
    );
    void update({ refinementModes: next });
  }

  function deleteMode(id: string) {
    const next = settings.refinementModes.filter((m) => m.id !== id);
    const nextQuick = settings.quickSwitchModeIds.filter((q) => q !== id);
    const nextActive =
      settings.activeRefinementModeId === id
        ? (next[0]?.id ?? null)
        : settings.activeRefinementModeId;
    void update({
      refinementModes: next,
      quickSwitchModeIds: nextQuick,
      activeRefinementModeId: nextActive,
    });
    setPendingId(next[0]?.id ?? null);
  }

  function addCustomMode() {
    const id = `custom-${Date.now().toString(36)}`;
    const newMode: RefinementMode = {
      id,
      name: t("home.refinementModeNewName"),
      emoji: "🔧",
      prompt: "",
      builtin: false,
    };
    void update({
      refinementModes: [...settings.refinementModes, newMode],
      quickSwitchModeIds: [...settings.quickSwitchModeIds, id],
      activeRefinementModeId: id,
    });
    setPendingId(id);
  }

  function toggleQuickSwitch(id: string) {
    const inCycle = settings.quickSwitchModeIds.includes(id);
    const next = inCycle
      ? settings.quickSwitchModeIds.filter((q) => q !== id)
      : [...settings.quickSwitchModeIds, id];
    void update({ quickSwitchModeIds: next });
  }

  return (
    <SettingsBento
      title={t("home.refinementTitle")}
      description={t("home.refinementDescription")}
      controlLabel={t("home.refinementActiveLabel")}
      disabled={!enabled}
      disabledNote={t("home.refinementModeOff")}
      control={
        <Switch
          checked={enabled}
          onChange={toggleEnabled}
          aria-label={t("home.refinementActiveLabel")}
        />
      }
    >
      <div className="grid grid-cols-[180px_1fr] divide-x divide-[var(--vk-border-3)]">
        <aside className="p-2 bg-[var(--vk-sidebar)]">
          <ul className="flex flex-col gap-0.5" role="list">
            {settings.refinementModes.map((mode) => (
              <li key={mode.id}>
                <SidebarRow
                  mode={mode}
                  isSelected={selectedMode?.id === mode.id}
                  onSelect={() => selectMode(mode.id)}
                />
              </li>
            ))}
          </ul>
          <div className="my-1.5 h-px bg-[var(--vk-border-3)]" />
          <button
            type="button"
            onClick={addCustomMode}
            className="w-full inline-flex items-center gap-2 px-3 py-2 rounded-md text-left text-[12px] text-[var(--vk-text-8)] hover:bg-[var(--vk-canvas-3)] hover:text-[var(--vk-text-4)] transition-colors"
          >
            <Plus className="size-3.5" aria-hidden />
            {t("home.refinementModeAdd")}
          </button>
        </aside>

        <div className="min-w-0">
          {selectedMode ? (
            <ModeEditor
              mode={selectedMode}
              translateTarget={settings.translateTarget}
              isInQuickSwitch={settings.quickSwitchModeIds.includes(
                selectedMode.id,
              )}
              onChange={(patch) => updateMode(selectedMode.id, patch)}
              onChangeTranslateTarget={(v) =>
                void update({ translateTarget: v })
              }
              onToggleQuickSwitch={() => toggleQuickSwitch(selectedMode.id)}
              onDelete={
                selectedMode.builtin
                  ? undefined
                  : () => deleteMode(selectedMode.id)
              }
              defaultPrompt={defaultPromptForBuiltin(selectedMode.id)}
            />
          ) : (
            <div className="py-6 px-4 text-center text-[12px] text-[var(--vk-text-8)]">
              {t("home.refinementModesEditorEmpty")}
            </div>
          )}
        </div>
      </div>

      <BentoBlock>
        <div className="text-[11.5px] text-[var(--vk-text-8)]">
          {t("home.refinementQuickSwitchHint")}
        </div>
      </BentoBlock>
    </SettingsBento>
  );
}

/// Returns the shipped default prompt for a built-in mode id so the
/// PromptEditor's "Reset" button has a target value. Sourced from the
/// canonical `DEFAULT_REFINEMENT_MODES` in settings.ts — when defaults
/// change, this updates automatically. Custom modes have no shipped
/// default — Reset is hidden for them.
function defaultPromptForBuiltin(id: string): string | null {
  return DEFAULT_REFINEMENT_MODES.find((m) => m.id === id)?.prompt ?? null;
}

// Sidebar row — full-width button styled like macOS Settings nav rows.
// Selected row gets a soft accent background instead of a saturated fill
// so it reads as "highlighted" without competing with the editor content
// on the right.
function SidebarRow({
  mode,
  isSelected,
  onSelect,
}: {
  mode: RefinementMode;
  isSelected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-pressed={isSelected}
      className={cn(
        "w-full flex items-center gap-2 px-3 py-2 rounded-md text-left text-[12.5px]",
        "transition-colors duration-150 outline-none",
        "focus-visible:ring-2 focus-visible:ring-[var(--vk-accent-4)]",
        isSelected
          ? "bg-[var(--vk-accent-soft-bg)] text-[var(--vk-accent-2)] font-medium"
          : "text-[var(--vk-text-4)] hover:bg-[var(--vk-canvas-3)]",
      )}
    >
      <span aria-hidden className="text-[14px] leading-none">
        {mode.emoji}
      </span>
      <span className="truncate">{mode.name}</span>
    </button>
  );
}

// Editor for the currently-selected mode. Renders directly inside the
// modes bento so its border/padding rhythm matches the rest of the body.
// Mode-specific config (e.g. Translate's target language) is gated on
// `mode.id` here instead of leaking to the bento root.
function ModeEditor({
  mode,
  translateTarget,
  isInQuickSwitch,
  onChange,
  onChangeTranslateTarget,
  onToggleQuickSwitch,
  onDelete,
  defaultPrompt,
}: {
  mode: RefinementMode;
  translateTarget: string;
  isInQuickSwitch: boolean;
  onChange: (patch: Partial<RefinementMode>) => void;
  onChangeTranslateTarget: (next: string) => void;
  onToggleQuickSwitch: () => void;
  onDelete?: () => void;
  defaultPrompt: string | null;
}) {
  const { t } = useTranslation();
  return (
    <>
      <BentoBlock>
        <div className="flex items-center gap-2">
          <InputText
            value={mode.emoji}
            onChange={(v) => onChange({ emoji: v })}
            placeholder="✨"
            className="w-12 text-center"
          />
          <InputText
            value={mode.name}
            onChange={(v) => onChange({ name: v })}
            placeholder={t("home.refinementModeNameLabel")}
            className="flex-1"
          />
          {onDelete ? (
            <button
              type="button"
              onClick={onDelete}
              className="h-8 px-2 rounded-md border border-transparent bg-transparent text-[12px] text-[var(--vk-danger)] hover:bg-[var(--vk-danger-soft-bg-2)] transition-colors"
              aria-label={t("common.delete")}
            >
              <Trash2 className="size-3.5" />
            </button>
          ) : null}
        </div>
      </BentoBlock>

      {mode.id === "translate" ? (
        <BentoRow
          label={t("home.translationTargetLanguage")}
          hint={t("home.refinementModeTranslateHint")}
        >
          <Select
            value={translateTarget}
            onChange={onChangeTranslateTarget}
            options={TRANSLATE_TARGETS}
          />
        </BentoRow>
      ) : null}

      <PromptEditor
        label={t("home.refinementModePromptLabel")}
        value={mode.prompt}
        defaultValue={defaultPrompt ?? mode.prompt}
        onChange={(v) =>
          // PromptEditor sends "" when the user resets to the default; that
          // would wipe a custom mode's prompt entirely. For built-ins,
          // empty means "use default" which we restore here; for custom
          // modes we keep whatever the user typed.
          onChange({
            prompt: v.length === 0 && defaultPrompt ? defaultPrompt : v,
          })
        }
        rows={12}
        placeholderTokens={
          mode.id === "translate" ? ["{target}"] : undefined
        }
      />

      <BentoRow label={t("home.refinementModeQuickSwitchToggle")}>
        <Switch
          checked={isInQuickSwitch}
          onChange={onToggleQuickSwitch}
          aria-label={t("home.refinementModeQuickSwitchToggle")}
        />
      </BentoRow>
    </>
  );
}

function Header({
  status,
  statusLabel,
  translateOn,
  translateTarget,
}: {
  status: Status;
  statusLabel: string;
  translateOn: boolean;
  translateTarget: string;
}) {
  const { t } = useTranslation();
  return (
    <header className="flex items-end justify-between gap-6 pb-8 border-b border-[var(--vk-border-2)]">
      <div className="flex items-center gap-3">
        <BrandMark />
        <div>
          <h1 className="text-[20px] font-semibold tracking-tight text-[var(--vk-text)] leading-none">
            Vibeking
          </h1>
          <p className="mt-1.5 text-[12px] text-[var(--vk-text-7)] leading-none">
            {t("app.tagline")}
          </p>
        </div>
      </div>
      <div className="flex items-center gap-2">
        <TranslateBadge on={translateOn} target={translateTarget} />
        <StatusPill status={status} label={statusLabel} />
      </div>
    </header>
  );
}

function BrandMark() {
  return <BrandChip size="md" />;
}

function StatsStrip({ hotwordCount }: { hotwordCount: number }) {
  const { t } = useTranslation();
  const [stats, setStats] = useState({ todayChars: 0, totalChars: 0 });

  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      void getStats()
        .then((s) => {
          if (!cancelled) setStats(s);
        })
        .catch(() => {});
    };
    refresh();
    window.addEventListener("vibeking:history-updated", refresh);
    return () => {
      cancelled = true;
      window.removeEventListener("vibeking:history-updated", refresh);
    };
  }, []);

  // Hierarchy: today is the primary action signal (varies per session,
  // tells the user "yes, I dictated"); total is a cumulative reference;
  // hotwords is settings context. The visual treatment matches: today
  // sits larger and darker, the other two are quiet supporting numbers
  // on the same baseline.
  return (
    <div className="mt-5 mb-4 flex items-baseline gap-3 text-[12px] text-[var(--vk-text-7)] leading-none">
      <span className="inline-flex items-baseline gap-1.5">
        <span className="text-[18px] font-semibold tracking-tight tabular-nums text-[var(--vk-text)] leading-none">
          {stats.todayChars.toLocaleString()}
        </span>
        <span>{t("home.todayWords")}</span>
      </span>
      <Sep />
      <SecondaryStat label={t("home.totalWords")} value={stats.totalChars} />
      <Sep />
      <SecondaryStat label={t("nav.hotwords")} value={hotwordCount} />
    </div>
  );
}

function SecondaryStat({ label, value }: { label: string; value: number }) {
  return (
    <span className="inline-flex items-baseline gap-1.5">
      <span className="tabular-nums font-medium text-[var(--vk-text-5)]">
        {value.toLocaleString()}
      </span>
      <span>{label}</span>
    </span>
  );
}

function Sep() {
  return <span className="text-[var(--vk-text-12)]">·</span>;
}

function MicrophoneSelect({
  value,
  onChange,
  systemLabel,
}: {
  value: string;
  onChange: (v: string) => void;
  systemLabel: string;
}) {
  const [devices, setDevices] = useState<string[]>([]);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    const refresh = () => {
      void listInputDevices()
        .then((list) => {
          if (!cancelled) setDevices(list);
        })
        .catch((err) =>
          console.warn("[vibeking] failed to list input devices:", err),
        );
    };
    // Subscribe before requesting the cached list so a background scan that
    // completes during mounting cannot leave the microphone menu stale.
    let unlisten: UnlistenFn | undefined;
    void listen<string[]>("audio:input-devices-changed", (event) => {
      if (!cancelled) setDevices(event.payload);
    }).then((stop) => {
      if (cancelled) stop();
      else { unlisten = stop; refresh(); }
    }).catch((error) => {
      console.warn("[vibeking] microphone updates unavailable:", error);
      if (!cancelled) refresh();
    });

    // Refresh whenever the OS reports an audio device change.
    const md = navigator.mediaDevices;
    md?.addEventListener?.("devicechange", refresh);
    // Belt-and-suspenders: refresh on window focus so the list is fresh
    // after the user plugs something in while another app is foregrounded.
    window.addEventListener("focus", refresh);

    return () => {
      cancelled = true;
      unlisten?.();
      md?.removeEventListener?.("devicechange", refresh);
      window.removeEventListener("focus", refresh);
    };
  }, []);

  const options = [
    { value: "", label: systemLabel },
    ...devices.map((name) => ({ value: name, label: name })),
  ];
  // Surface a saved-but-missing device so the user can see and clear it.
  if (value && !devices.includes(value)) {
    options.push({ value, label: `${value} (unavailable)` });
  }

  return <Select value={value} onChange={onChange} options={options} />;
}

/**
 * Builds the hint shown under the STT-language Combobox. Composes the
 * three pieces the user cares about into one comma-joined line:
 *
 *   1. provider-specific note (Parakeet auto-detects, etc.)
 *   2. menubar-favorite count
 *   3. count of favorites the active provider can't transcribe (only
 *      surfaced when there's something to surface — otherwise the line
 *      stays calm)
 */
function sttLanguageHint(
  t: (k: string, opts?: Record<string, unknown>) => string,
  s: ReturnType<typeof useSettings>[0],
): string {
  const parts: string[] = [];
  parts.push(
    s.provider === "fluid-audio"
      ? t("home.parakeetLangAutoDetect")
      : t("home.sttLanguageHint"),
  );

  const favCount = s.favoriteSttLanguages.length;
  if (favCount > 0) {
    parts.push(t("home.sttLanguageFavoriteCount", { n: favCount }));
  }

  if (s.provider === "fluid-audio" && favCount > 0) {
    const hidden = s.favoriteSttLanguages.filter(
      (code) => !sttLanguageSupported("fluid-audio", code),
    ).length;
    if (hidden > 0) {
      parts.push(
        t("home.sttLanguageFavoriteHiddenByParakeet", { n: hidden }),
      );
    }
  }

  return parts.join(" · ");
}

// ---------------------------------------------------------------------------
// Debug: "What Vibeking sees"
// ---------------------------------------------------------------------------

/**
 * Only renders when the URL includes `?debug=1`. Shows a ring buffer of
 * the last 5 screen-context captures with metadata and the proper-noun
 * candidate list. The raw text is intentionally NOT included on the
 * Tauri event bus.
 */
function DebugContextPanel() {
  const { t } = useTranslation();
  const enabled =
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("debug") === "1";

  const [captures, setCaptures] = useState<ContextCapturedEvent[]>([]);
  const [, setTick] = useState(0);

  useEffect(() => {
    if (!enabled) return;
    const unsub = subscribeContextCaptured((e) => {
      setCaptures((prev) => [e, ...prev].slice(0, 5));
    });
    return () => unsub();
  }, [enabled]);

  // Keep the "time ago" labels live by ticking every 10s while mounted.
  useEffect(() => {
    if (!enabled) return;
    const id = window.setInterval(() => setTick((n) => n + 1), 10_000);
    return () => window.clearInterval(id);
  }, [enabled]);

  if (!enabled) return null;

  return (
    <Section
      eyebrow={t("home.debugLabel")}
      title={t("home.debugTitle")}
      description="Last 5 screen-context captures. Raw text never leaves the Rust process — only proper-noun candidates are shown."
    >
      <SettingsCard>
        {captures.length === 0 ? (
          <div className="px-4 py-6 text-[12.5px] text-[var(--vk-text-8)]">
            No captures yet. Press the dictation hotkey to capture screen
            context.
          </div>
        ) : (
          captures.map((c, i) => <CaptureRow key={c.capturedAt + ":" + i} c={c} />)
        )}
      </SettingsCard>
    </Section>
  );
}

function CaptureRow({ c }: { c: ContextCapturedEvent }) {
  const visible = c.candidates.slice(0, 10);
  const overflow = Math.max(0, c.candidates.length - visible.length);
  const appLabel =
    c.app_name || c.bundle_id || (
      <span className="text-[var(--vk-text-8)]">unknown</span>
    );

  return (
    <div className="border-t border-[var(--vk-border-3)] first:border-t-0 px-4 py-3.5">
      <div className="flex items-baseline justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-medium text-[var(--vk-text-2)] truncate">
            {appLabel}
          </div>
          {c.bundle_id && c.app_name ? (
            <div className="mt-0.5 text-[11px] text-[var(--vk-text-8)] truncate font-mono">
              {c.bundle_id}
            </div>
          ) : null}
        </div>
        <div className="flex items-center gap-1.5 shrink-0">
          <SourceBadge source={c.source} />
          <span className="text-[11px] tabular-nums text-[var(--vk-text-7)]">
            {c.latency_ms}ms
          </span>
          <span className="text-[11px] tabular-nums text-[var(--vk-text-7)]">
            · {timeAgo(c.capturedAt)}
          </span>
        </div>
      </div>

      <div className="mt-2 text-[11.5px] text-[var(--vk-text-7)] tabular-nums">
        {c.candidates.length} candidate{c.candidates.length === 1 ? "" : "s"}
        {" · "}
        {c.raw_text_len.toLocaleString()} chars captured
      </div>

      {visible.length > 0 ? (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {visible.map((cand, idx) => (
            <span
              key={idx}
              className="inline-flex items-center h-5 px-1.5 rounded text-[10.5px] font-mono bg-[var(--vk-surface-2)] border border-[var(--vk-border)] text-[var(--vk-text-3)]"
            >
              {cand}
            </span>
          ))}
          {overflow > 0 ? (
            <span className="inline-flex items-center h-5 px-1.5 rounded text-[10.5px] text-[var(--vk-text-7)]">
              +{overflow} more
            </span>
          ) : null}
        </div>
      ) : (
        <div className="mt-2 text-[11px] text-[var(--vk-text-8)] italic">
          No proper-noun candidates extracted.
        </div>
      )}
    </div>
  );
}

function SourceBadge({ source }: { source: "ax" | "ocr" }) {
  const isAx = source === "ax";
  return (
    <span
      className={
        "inline-flex items-center h-4 px-1.5 rounded text-[9.5px] font-semibold uppercase tracking-wider " +
        (isAx
          ? "bg-[var(--vk-info-soft-bg)] text-[var(--vk-info-fg)]"
          : "bg-[var(--vk-danger-soft-bg-5)] text-[var(--vk-danger-8)]")
      }
    >
      {source}
    </span>
  );
}

function timeAgo(ts: number): string {
  const seconds = Math.max(0, Math.round((Date.now() - ts) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  return `${hours}h ago`;
}
