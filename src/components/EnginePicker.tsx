import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Download, Loader2, Trash2 } from "lucide-react";
import { cn } from "@/lib/utils";
import type { Provider, Settings } from "@/lib/settings";
import {
  englishLanguageNames,
  sttLanguageSupported,
} from "@/lib/stt-languages";
import { useIsAppleSilicon } from "@/lib/platform";
import { type Engine } from "@/lib/local-model";
import {
  ON_DEVICE,
  CLOUD,
  useEngineStatus,
  type OnDeviceEngine,
  type CloudEngine,
  type Pill,
  type EngineStatus,
} from "@/lib/engines";
import { InputKey } from "@/components/ui/input-key";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { Switch } from "@/components/ui/switch";
import { Select } from "@/components/ui/select";

type Update = (patch: Partial<Settings>) => Promise<void>;

// Engine descriptors (ON_DEVICE / CLOUD) and the useEngineStatus hook now live
// in @/lib/engines so the onboarding wizard shares one source of truth.

// Widely-recognized languages shown before the full A–Z list, so a user sees
// familiar names immediately and only expands if theirs isn't among them.
const POPULAR_FIRST = [
  "English", "Chinese", "Cantonese", "Spanish", "French", "German",
  "Japanese", "Korean", "Portuguese", "Italian", "Russian", "Arabic",
  "Hindi", "Vietnamese", "Thai", "Turkish", "Dutch", "Polish",
];

const POPULAR_COUNT = 6;

function pickPopular(all: string[], n: number): string[] {
  const set = new Set(all);
  const picked: string[] = [];
  for (const name of POPULAR_FIRST) {
    if (set.has(name)) picked.push(name);
    if (picked.length >= n) break;
  }
  // Top up from the alphabetical list when fewer than n popular ones fit.
  for (const name of all) {
    if (picked.length >= n) break;
    if (!picked.includes(name)) picked.push(name);
  }
  return picked;
}

// ---------------------------------------------------------------------------
// Pills + bullet list
// ---------------------------------------------------------------------------

function PillBadge({ pill }: { pill: Pill }) {
  const { t } = useTranslation();
  const tone =
    pill.tone === "accent"
      ? "text-[var(--vk-accent-2)] bg-[var(--vk-accent-soft-bg)]"
      : pill.tone === "amber"
        ? "text-[oklch(0.5_0.13_70)] bg-[oklch(0.95_0.05_80)]"
        : "text-[var(--vk-text-6)] bg-[var(--vk-surface-3)]";
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-full px-1.5 py-0.5",
        "text-[10.5px] font-medium leading-none tracking-tight",
        tone,
      )}
    >
      {t(pill.key)}
    </span>
  );
}

function BulletList({ children }: { children: React.ReactNode }) {
  // Proper "•" markers via a styled <ul>: list-style none + a pseudo-dot so
  // the bullets sit in a fixed gutter and wrapped lines align under the text.
  return (
    <ul className="mt-1.5 space-y-1 text-[12px] leading-relaxed">{children}</ul>
  );
}

function Bullet({ children }: { children: React.ReactNode }) {
  return (
    <li className="relative pl-3.5 before:absolute before:left-0 before:top-0 before:text-[var(--vk-text-10)] before:content-['•']">
      {children}
    </li>
  );
}

// ---------------------------------------------------------------------------
// Right-side status / action area for an on-device card.
// ---------------------------------------------------------------------------

function EngineAction({
  engine,
  status,
}: {
  engine: Engine;
  status: EngineStatus;
}) {
  const { t } = useTranslation();

  // Two-step inline confirm for Clear. window.confirm() is unreliable in
  // Tauri 2, so the button itself becomes a red "Confirm?" for 3s; a second
  // click within that window deletes. Auto-resets otherwise.
  const [confirming, setConfirming] = useState(false);
  const [clearing, setClearing] = useState(false);
  useEffect(() => {
    if (!confirming) return;
    const id = window.setTimeout(() => setConfirming(false), 3000);
    return () => window.clearTimeout(id);
  }, [confirming]);

  // Stop click from also toggling the card selection.
  const stop = (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
  };

  if (!status.inTauri) {
    return (
      <span className="text-[11.5px] text-[var(--vk-text-6)]">
        {t("home.enginePicker.desktopOnly")}
      </span>
    );
  }

  if (status.busy) {
    // Whisper reports a real % (determinate bar); the others are indeterminate.
    if (status.pct !== null) {
      return (
        <div className="flex w-[120px] flex-col items-end gap-1">
          <ProgressBar value={status.pct} />
          <span className="text-[10.5px] tabular-nums text-[var(--vk-text-6)]">
            {status.pct}%
          </span>
        </div>
      );
    }
    return (
      <span className="inline-flex items-center gap-1.5 text-[11.5px] text-[var(--vk-text-6)]">
        <Loader2 className="size-3.5 animate-spin motion-reduce:hidden" />
        {t("home.enginePicker.preparing")}
      </span>
    );
  }

  if (status.ready) {
    async function onClear(e: React.MouseEvent) {
      stop(e);
      if (!confirming) {
        setConfirming(true);
        return;
      }
      setConfirming(false);
      setClearing(true);
      try {
        await status.remove();
      } finally {
        setClearing(false);
      }
    }
    return (
      <div className="inline-flex items-center gap-2">
        <span className="inline-flex items-center gap-1.5 text-[11.5px] font-medium text-[var(--vk-success-2)]">
          <span
            aria-hidden
            className="inline-block size-1.5 rounded-full bg-[var(--vk-success-dot)]"
          />
          {t("home.enginePicker.ready")}
        </span>
        <button
          type="button"
          onClick={onClear}
          disabled={clearing}
          aria-label={
            confirming ? t("home.enginePicker.confirm") : t("home.enginePicker.clear")
          }
          className={cn(
            "inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[11.5px] font-medium",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[--color-ring]",
            "transition-colors motion-reduce:transition-none disabled:opacity-60",
            confirming
              ? "text-[var(--vk-danger-4)]"
              : "text-[var(--vk-text-6)] hover:text-[var(--vk-danger-4)]",
          )}
        >
          {clearing ? (
            <Loader2 className="size-3 animate-spin motion-reduce:hidden" />
          ) : (
            <Trash2 className="size-3" />
          )}
          {confirming ? t("home.enginePicker.confirm") : t("home.enginePicker.clear")}
        </button>
      </div>
    );
  }

  // Not downloaded.
  void engine;
  return (
    <button
      type="button"
      onClick={(e) => {
        stop(e);
        void status.download();
      }}
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md px-1.5 py-0.5 text-[12px] font-medium",
        "text-[var(--vk-accent-2)] hover:text-[var(--vk-accent-3)]",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[--color-ring]",
        "transition-colors motion-reduce:transition-none",
      )}
    >
      <Download className="size-3.5" />
      {t("home.enginePicker.download")}
    </button>
  );
}

// ---------------------------------------------------------------------------
// On-device card. Exported so an onboarding flow can reuse it standalone.
// ---------------------------------------------------------------------------

export function EngineCard({
  spec,
  selected,
  disabled,
  onSelect,
  settings,
  update,
}: {
  spec: OnDeviceEngine;
  selected: boolean;
  disabled: boolean;
  onSelect: () => void;
  settings: Settings;
  update: Update;
}) {
  const { t } = useTranslation();
  const status = useEngineStatus(spec.engine);
  const allLangs = englishLanguageNames(spec.engine);
  const popular = pickPopular(allLangs, POPULAR_COUNT);
  const hiddenCount = allLangs.length - popular.length;
  const [showAllLangs, setShowAllLangs] = useState(false);

  return (
    <div
      className={cn(
        "relative rounded-2xl bg-[var(--vk-surface)] px-4 py-4",
        "transition-[box-shadow,border-color] duration-150 ease-out motion-reduce:transition-none",
        selected
          ? "border border-[var(--vk-text-2)] shadow-[0_2px_4px_var(--vk-shadow-04),0_8px_20px_var(--vk-shadow-05)]"
          : "border border-[var(--vk-border-3)] shadow-[0_1px_2px_var(--vk-shadow-03),0_4px_12px_var(--vk-shadow-04)]",
        !selected && !disabled &&
          "hover:shadow-[0_2px_6px_var(--vk-shadow-04),0_10px_24px_var(--vk-shadow-05)]",
        disabled && "opacity-55",
      )}
    >
      {/* Full-card selection target: a real <button> stretched over the card,
          so keyboard + screen-reader semantics are native and there's no
          invalid button-in-button nesting. Its accessible name is just the
          engine name; the bullets read as separate content. Interactive
          children (Clear / Download / View all) opt back in via
          pointer-events + z-index. */}
      <button
        type="button"
        onClick={disabled ? undefined : onSelect}
        disabled={disabled}
        aria-pressed={selected}
        aria-label={spec.name}
        className={cn(
          "absolute inset-0 rounded-2xl",
          "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[--color-ring] focus-visible:ring-inset",
          disabled ? "cursor-not-allowed" : "cursor-pointer",
        )}
      />

      <div className="pointer-events-none relative flex items-start gap-2.5">
        {/* Reserved 22px check slot — no layout shift between states. */}
        <span className="mt-0.5 flex w-[22px] shrink-0 items-center justify-center">
          {selected ? (
            <span className="inline-flex size-[18px] items-center justify-center rounded-full bg-[var(--vk-success-soft-bg)]">
              <Check className="size-3 text-[var(--vk-success-2)]" />
            </span>
          ) : null}
        </span>

        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="text-[15.5px] font-[640] text-[var(--vk-text)]">
              {spec.name}
            </span>
            {spec.pills.map((p) => (
              <PillBadge key={p.key} pill={p} />
            ))}
            <span className="flex-1" />
            {disabled ? (
              <span className="text-[11.5px] text-[var(--vk-text-6)]">
                {t("home.enginePicker.needsAppleSilicon")}
              </span>
            ) : (
              <span className="pointer-events-auto relative z-10 shrink-0">
                <EngineAction engine={spec.engine} status={status} />
              </span>
            )}
          </div>

          <BulletList>
            <Bullet>
              <span className="text-[var(--vk-text-4)]">{t(spec.bestForKey)}</span>
            </Bullet>
            <Bullet>
              <span className="text-[var(--vk-text-6)]">
                {t("home.enginePicker.languagesLabel")}{" "}
                {showAllLangs ? allLangs.join(", ") : popular.join(", ")}
                {hiddenCount > 0 ? (
                  <>
                    {" "}
                    <button
                      type="button"
                      onClick={(e) => {
                        e.stopPropagation();
                        setShowAllLangs((v) => !v);
                      }}
                      className="pointer-events-auto relative z-10 font-medium text-[var(--vk-accent-2)] hover:text-[var(--vk-accent-3)] focus-visible:underline focus-visible:outline-none"
                    >
                      {showAllLangs
                        ? t("home.enginePicker.showFewer")
                        : t("home.enginePicker.viewAll", {
                            count: allLangs.length,
                          })}
                    </button>
                  </>
                ) : null}
              </span>
            </Bullet>
            <Bullet>
              <span className="text-[var(--vk-text-6)]">{t(spec.sizeKey)}</span>
            </Bullet>
          </BulletList>

          {status.error ? (
            <p className="pointer-events-auto relative z-10 mt-1.5 pl-3.5 text-[11.5px] text-[var(--vk-danger-4)]">
              {status.error}
            </p>
          ) : null}

          {/* Memory policy — Gemma is the only on-device engine that unloads
              when idle (it's the heavy ~4-6 GB external sidecar). Shown inside
              its own card, only once downloaded, so it doesn't clutter the
              shared STT section or appear before there's anything to unload. */}
          {spec.provider === "gemma" && status.ready ? (
            <GemmaMemoryControl settings={settings} update={update} />
          ) : null}
        </div>
      </div>
    </div>
  );
}

// Keep-loaded toggle + idle-unload timeout, rendered inside the Gemma card.
function GemmaMemoryControl({
  settings,
  update,
}: {
  settings: Settings;
  update: Update;
}) {
  const { t } = useTranslation();
  return (
    <div className="pointer-events-auto relative z-10 mt-3 space-y-2 border-t border-[var(--vk-border-3)] pl-3.5 pt-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="text-[12.5px] font-medium text-[var(--vk-text-3)]">
            {t("home.gemmaKeepLoaded")}
          </div>
          <div className="mt-0.5 text-[10.5px] leading-snug text-[var(--vk-text-7)]">
            {t("home.gemmaKeepLoadedHint")}
          </div>
        </div>
        <Switch
          checked={settings.gemmaKeepLoaded}
          onChange={(on) => void update({ gemmaKeepLoaded: on })}
          aria-label={t("home.gemmaKeepLoaded")}
        />
      </div>
      {!settings.gemmaKeepLoaded ? (
        <div className="flex items-center justify-between gap-3">
          <div className="text-[12.5px] font-medium text-[var(--vk-text-3)]">
            {t("home.gemmaIdleTimeout")}
          </div>
          <Select
            value={String(settings.gemmaIdleTimeoutMin)}
            onChange={(v) => void update({ gemmaIdleTimeoutMin: Number(v) })}
            options={[
              { value: "5", label: t("home.gemmaIdleMinutes", { n: 5 }) },
              { value: "15", label: t("home.gemmaIdleMinutes", { n: 15 }) },
              { value: "30", label: t("home.gemmaIdleMinutes", { n: 30 }) },
              { value: "60", label: t("home.gemmaIdleMinutes", { n: 60 }) },
            ]}
          />
        </div>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Cloud row + revealed key input.
// ---------------------------------------------------------------------------

function CloudRow({
  spec,
  selected,
  apiKey,
  onSelect,
  onKey,
}: {
  spec: CloudEngine;
  selected: boolean;
  apiKey: string;
  onSelect: () => void;
  onKey: (v: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <div
      className={cn(
        "rounded-2xl border bg-[var(--vk-surface)]",
        "transition-shadow duration-150 ease-out motion-reduce:transition-none",
        selected
          ? "border-[var(--vk-text-2)] shadow-[0_2px_4px_var(--vk-shadow-04),0_8px_20px_var(--vk-shadow-05)]"
          : "border-[var(--vk-border-3)] shadow-[0_1px_2px_var(--vk-shadow-03),0_4px_12px_var(--vk-shadow-04)]",
      )}
    >
      <button
        type="button"
        onClick={onSelect}
        aria-pressed={selected}
        className={cn(
          "flex w-full items-center gap-2.5 rounded-2xl px-4 py-3 text-left",
          "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[--color-ring] focus-visible:ring-offset-1",
          !selected && "hover:bg-[var(--vk-canvas-3)]",
          "transition-colors motion-reduce:transition-none",
        )}
      >
        <span className="flex w-[22px] shrink-0 items-center justify-center">
          {selected ? (
            <span className="inline-flex size-[18px] items-center justify-center rounded-full bg-[var(--vk-success-soft-bg)]">
              <Check className="size-3 text-[var(--vk-success-2)]" />
            </span>
          ) : null}
        </span>
        <span className="text-[14px] font-medium text-[var(--vk-text-2)]">
          {spec.name}
        </span>
        <span className="flex-1" />
        {!selected ? (
          <span className="text-[12px] font-medium text-[var(--vk-accent-2)]">
            {t("home.enginePicker.addKey")} →
          </span>
        ) : null}
      </button>
      {selected ? (
        <div className="flex items-center justify-between gap-3 border-t border-[var(--vk-border-3)] px-3.5 py-2.5 pl-[46px]">
          <span className="text-[12px] text-[var(--vk-text-6)]">
            {t("home.bentoApiKey")}
          </span>
          <InputKey value={apiKey} onChange={onKey} />
        </div>
      ) : null}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Group header (small uppercase label + one-line note).
// ---------------------------------------------------------------------------

function GroupHeader({ label, note }: { label: string; note: string }) {
  return (
    <div className="mb-2.5 px-1">
      <div className="text-[10.5px] font-semibold uppercase tracking-[0.12em] text-[var(--vk-text-eyebrow)]">
        {label}
      </div>
      <p className="mt-1 text-[12px] leading-relaxed text-[var(--vk-text-6)]">
        {note}
      </p>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Cloud provider key plumbing — mirrors Home.tsx's apiKeyFor / patchKey for
// the four cloud providers (on-device providers have no key).
// ---------------------------------------------------------------------------

function cloudKeyFor(s: Settings, p: Provider): string {
  switch (p) {
    case "deepgram":
      return s.deepgramKey;
    case "groq":
      return s.groqKey;
    case "openai":
      return s.openaiKey;
    case "elevenlabs":
      return s.elevenlabsKey;
    default:
      return "";
  }
}

function patchCloudKey(p: Provider, v: string): Partial<Settings> {
  switch (p) {
    case "deepgram":
      return { deepgramKey: v };
    case "groq":
      return { groqKey: v };
    case "openai":
      return { openaiKey: v };
    case "elevenlabs":
      return { elevenlabsKey: v };
    default:
      return {};
  }
}

// ---------------------------------------------------------------------------
// EnginePicker — the drop-in replacement for the old ProviderRow + 4 rows.
// ---------------------------------------------------------------------------

export function EnginePicker({
  settings,
  update,
}: {
  settings: Settings;
  update: Update;
}) {
  const { t } = useTranslation();
  const isAS = useIsAppleSilicon();
  // Until the platform probe resolves, treat as Apple Silicon (the common
  // case) to avoid a flash-of-disabled state.
  const appleSiliconDisabled = isAS === false;

  function select(provider: Provider) {
    const patch: Partial<Settings> = { provider };
    if (!sttLanguageSupported(provider, settings.sttLanguage)) {
      patch.sttLanguage = "auto";
    }
    void update(patch);
  }

  return (
    <div className="px-4 py-4">
      {/* On your Mac */}
      <GroupHeader
        label={t("home.enginePicker.onDeviceGroup")}
        note={t("home.enginePicker.onDeviceNote")}
      />
      <div className="space-y-2">
        {ON_DEVICE.map((spec) => {
          const disabled = spec.requiresAppleSilicon && appleSiliconDisabled;
          return (
            <EngineCard
              key={spec.provider}
              spec={spec}
              selected={settings.provider === spec.provider}
              disabled={disabled}
              onSelect={() => select(spec.provider)}
              settings={settings}
              update={update}
            />
          );
        })}
      </div>

      {/* Online */}
      <div className="mt-5">
        <GroupHeader
          label={t("home.enginePicker.cloudGroup")}
          note={t("home.enginePicker.cloudNote")}
        />
        <div className="space-y-2">
          {CLOUD.map((spec) => (
            <CloudRow
              key={spec.provider}
              spec={spec}
              selected={settings.provider === spec.provider}
              apiKey={cloudKeyFor(settings, spec.provider)}
              onSelect={() => select(spec.provider)}
              onKey={(v) => void update(patchCloudKey(spec.provider, v))}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
