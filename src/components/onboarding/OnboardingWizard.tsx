import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import {
  ArrowLeft,
  Check,
  Cloud,
  Laptop,
  Loader2,
  Lock,
  Zap,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { isTauri } from "@/lib/runtime";
import { useSettings } from "@/lib/use-settings";
import { useIsAppleSilicon } from "@/lib/platform";
import type { Provider, Settings } from "@/lib/settings";
import { listInputDevices } from "@/lib/settings";
import {
  downloadModel,
  getModelStatus,
  subscribeModelComplete,
  subscribeModelProgress,
  useIsPreparing,
  type Engine,
} from "@/lib/local-model";
import { ON_DEVICE, type OnDeviceEngine, type Pill } from "@/lib/engines";
import {
  checkPermissions,
  openSettingsPane,
  type PermissionsStatus,
  type SettingsPane,
} from "@/lib/permissions";
import {
  startMicMonitor,
  stopMicMonitor,
  subscribeAudioLevel,
} from "@/lib/audio-level";
import { BrandChip } from "@/components/BrandMark";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { formatHotkey } from "@/components/ui/hotkey-recorder";

// ---------------------------------------------------------------------------
// OnboardingWizard — the first-run flow. A step-by-step wizard (top progress
// track, ← Back, bold left title + focused right card, one action per step)
// rendered in Vibeking's design language. Replaces the single OnboardingCard
// that used to live in PermissionsGate. Calls `onComplete` (which persists the
// completion flag) from the final step.
//
// Steps are conditional on the engine choice. Cloud commits on the first screen
// and skips straight to permissions; local gets a dedicated engine-pick screen
// so the 2×2 of on-device engines has room to breathe:
//   cloud:  engine → permissions → mic → done                         (4)
//   local:  engine → engine-pick → download → permissions → mic → done (6)
// ---------------------------------------------------------------------------

type StepId =
  | "engine"
  | "engine-pick"
  | "download"
  | "permissions"
  | "mic"
  | "done";
type Choice = "cloud" | "local";

// The onboarding flow is a deliberately light, bold surface (the approved
// design). The app supports dark mode, which flips the global `--vk-*` tokens
// on `html.dark` — that would make this screen light-bg + dark-cards +
// light-text (title invisible). Re-declaring the light token values on the
// wizard root pins the whole subtree to the light look: a closer custom-prop
// scope wins over `html.dark`, so children keep using tokens normally and
// always resolve light. `colorScheme: light` keeps form controls light too.
const LIGHT_THEME = {
  colorScheme: "light",
  "--vk-canvas-3": "oklch(0.985 0.002 268)",
  "--vk-surface": "oklch(1 0 0)",
  "--vk-surface-2": "oklch(0.97 0.005 268)",
  "--vk-surface-3": "oklch(0.96 0.005 268)",
  "--vk-border": "oklch(0.92 0.005 268)",
  "--vk-border-3": "oklch(0.95 0.005 268)",
  "--vk-border-strong": "oklch(0.9 0.005 268)",
  "--vk-text": "oklch(0.16 0.01 268)",
  "--vk-text-2": "oklch(0.2 0.01 268)",
  "--vk-text-4": "oklch(0.3 0.01 268)",
  "--vk-text-7": "oklch(0.5 0.01 268)",
  "--vk-text-8": "oklch(0.55 0.01 268)",
  "--vk-text-9": "oklch(0.6 0.01 268)",
  "--vk-accent": "oklch(0.55 0.2 268)",
  "--vk-accent-2": "oklch(0.55 0.18 268)",
  "--vk-accent-3": "oklch(0.5 0.2 268)",
  "--vk-accent-9": "oklch(0.7 0.16 268)",
  "--vk-accent-pop-2": "oklch(0.55 0.18 278)",
  "--vk-accent-soft-bg": "oklch(0.95 0.03 268)",
  "--vk-accent-soft-bg-3": "oklch(0.97 0.04 268)",
  "--vk-success-2": "oklch(0.45 0.16 152)",
  "--vk-success-soft-bg": "oklch(0.95 0.04 152)",
  "--vk-info-soft-bg": "oklch(0.95 0.04 240)",
  "--vk-info-fg": "oklch(0.4 0.15 240)",
  "--vk-shadow-04": "oklch(0 0 0 / 0.04)",
  "--vk-shadow-05": "oklch(0 0 0 / 0.05)",
  "--vk-shadow-08": "oklch(0 0 0 / 0.08)",
} as React.CSSProperties;

export function OnboardingWizard({ onComplete }: { onComplete: () => void }) {
  const { t } = useTranslation();
  const [settings, update] = useSettings();
  const isAppleSilicon = useIsAppleSilicon();

  const [choice, setChoice] = useState<Choice | null>(null);
  const [idx, setIdx] = useState(0);

  // On-device engine: the user PICKS one from a 2×2 grid in the engine step.
  // The default highlight follows hardware — Parakeet on Apple Silicon, Whisper
  // on Intel (the only engine that runs there). `localEngine === null` means the
  // user hasn't overridden the default, so we track the hardware default live
  // until they do.
  const appleSiliconDisabled = isAppleSilicon === false;
  // Default highlight follows hardware AND the recommendation: the recommended
  // engine (Gemma) on Apple silicon, Whisper on Intel (the only engine that runs
  // there). `localEngine === null` means the user hasn't overridden it, so we
  // track this default live until they pick.
  const defaultEngine = useMemo<OnDeviceEngine>(
    () =>
      appleSiliconDisabled
        ? ON_DEVICE.find((e) => e.provider === "local")!
        : ON_DEVICE.find((e) => e.recommended)!,
    [appleSiliconDisabled],
  );
  const [localEngine, setLocalEngine] = useState<OnDeviceEngine | null>(null);
  // Never let a gated engine stay selected (e.g. the hardware probe resolves to
  // Intel after the user picked an Apple-silicon-only tile) — fall back to the
  // hardware default in that case.
  const selectedEngine =
    localEngine && !(localEngine.requiresAppleSilicon && appleSiliconDisabled)
      ? localEngine
      : defaultEngine;

  const localProvider: Provider = selectedEngine.provider;
  const dlEngine: Engine = selectedEngine.engine;
  const engineShort = selectedEngine.name;

  const steps: StepId[] = useMemo(
    () =>
      (choice ?? "local") === "local"
        ? ["engine", "engine-pick", "download", "permissions", "mic", "done"]
        : ["engine", "permissions", "mic", "done"],
    [choice],
  );
  const step = steps[idx];
  const total = steps.length;

  const goBack = useCallback(() => setIdx((i) => Math.max(0, i - 1)), []);
  const goNext = useCallback(
    () => setIdx((i) => Math.min(total - 1, i + 1)),
    [total],
  );

  // Cloud commits its provider here (Deepgram — already the fresh-install
  // default; set explicitly so a re-run lands in a known-good state) and skips
  // straight to permissions. Local advances to the dedicated engine-pick screen
  // WITHOUT committing — the engine is chosen and committed there.
  const continueFromEngine = useCallback(async () => {
    if (!choice) return;
    if (choice === "cloud") await update({ provider: "deepgram" });
    goNext();
  }, [choice, update, goNext]);

  // Commit the chosen on-device engine → provider, then advance to download.
  const commitLocalEngine = useCallback(async () => {
    await update({ provider: localProvider });
    goNext();
  }, [localProvider, update, goNext]);

  return (
    <div
      className="fixed inset-0 z-[100] overflow-hidden animate-vibeking-fade-up"
      style={LIGHT_THEME}
    >
      {/* Soft grey→lavender→blush wash behind the flow. */}
      <div
        className="absolute inset-0"
        style={{
          background:
            "radial-gradient(120% 90% at 88% 8%, oklch(0.97 0.03 320 / 0.6), transparent 55%)," +
            "radial-gradient(120% 120% at 0% 0%, oklch(0.985 0.004 268), transparent 60%)," +
            "linear-gradient(135deg, oklch(0.97 0.004 268) 0%, oklch(0.965 0.012 292) 58%, oklch(0.96 0.022 330) 100%)",
        }}
      />

      {/* Top progress track. */}
      <div className="absolute left-1/2 top-9 w-[300px] -translate-x-1/2">
        <ProgressBar
          value={((idx + 1) / total) * 100}
          gradient
          className="h-[5px] bg-[oklch(0.88_0.01_286_/_0.55)]"
        />
      </div>

      <div className="absolute inset-0 flex flex-col px-[68px] pt-16 pb-14">
        <button
          type="button"
          onClick={goBack}
          className={cn(
            "mb-6 -ml-1 inline-flex items-center gap-2 self-start rounded-lg px-1 py-1.5",
            "text-[15px] font-medium text-[var(--vk-text-2)] transition-opacity hover:opacity-60",
            idx === 0 && "invisible",
          )}
        >
          <ArrowLeft className="size-[18px]" />
          {t("onboarding.back")}
        </button>

        <div className="grid flex-1 grid-cols-[minmax(0,0.92fr)_minmax(0,1.08fr)] items-center gap-[54px]">
          {step === "engine" && (
            <EngineStep
              t={t}
              choice={choice}
              setChoice={setChoice}
              onContinue={continueFromEngine}
            />
          )}
          {step === "engine-pick" && (
            <EnginePickStep
              t={t}
              detecting={isAppleSilicon === null}
              appleSiliconDisabled={appleSiliconDisabled}
              selectedProvider={selectedEngine.provider}
              onPickEngine={setLocalEngine}
              onContinue={commitLocalEngine}
            />
          )}
          {step === "download" && (
            <DownloadStep
              t={t}
              engine={dlEngine}
              engineShort={engineShort}
              onNext={goNext}
            />
          )}
          {step === "permissions" && (
            <PermissionsStep t={t} onNext={goNext} />
          )}
          {step === "mic" && (
            <MicStep
              t={t}
              settings={settings}
              update={update}
              onNext={goNext}
            />
          )}
          {step === "done" && (
            <DoneStep t={t} settings={settings} onComplete={onComplete} />
          )}
        </div>
      </div>
    </div>
  );
}

// Shared layout: bold left lead + right card + bottom-right action.
function StepLayout({
  lead,
  card,
  action,
}: {
  lead: React.ReactNode;
  card: React.ReactNode;
  action: React.ReactNode;
}) {
  return (
    <>
      <div className="animate-vibeking-fade-up pb-4">{lead}</div>
      <div className="flex h-full flex-col justify-center animate-vibeking-fade-up">
        {card}
        <div className="mt-7 flex justify-end">{action}</div>
      </div>
    </>
  );
}

function Title({ children }: { children: React.ReactNode }) {
  return (
    <h1 className="text-[42px] font-[640] leading-[1.04] tracking-[-0.022em] text-[var(--vk-text)]">
      {children}
    </h1>
  );
}

function Desc({ children }: { children: React.ReactNode }) {
  return (
    <p className="mt-[18px] max-w-[36ch] text-[14.5px] leading-[1.55] text-[var(--vk-text-7)]">
      {children}
    </p>
  );
}

function PrimaryButton({
  onClick,
  disabled,
  children,
}: {
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={cn(
        "rounded-full px-[26px] py-3 text-[14.5px] font-semibold transition-colors",
        disabled
          ? "cursor-not-allowed bg-[var(--vk-surface-3)] text-[var(--vk-text-9)]"
          : "bg-[var(--vk-accent)] text-white shadow-[0_4px_14px_oklch(0.55_0.2_268_/_0.3)] hover:bg-[var(--vk-accent-3)]",
      )}
    >
      {children}
    </button>
  );
}

type TFn = ReturnType<typeof useTranslation>["t"];

// ── Step 1: cloud vs local ──────────────────────────────────────────────────
function EngineStep({
  t,
  choice,
  setChoice,
  onContinue,
}: {
  t: TFn;
  choice: Choice | null;
  setChoice: (c: Choice) => void;
  onContinue: () => void;
}) {
  return (
    <StepLayout
      lead={
        <>
          <Title>{t("onboarding.title")}</Title>
          <Desc>{t("onboarding.subtitle")}</Desc>
        </>
      }
      card={
        <div className="overflow-hidden rounded-[20px] border border-[var(--vk-border)] bg-[var(--vk-surface)] shadow-[0_1px_2px_var(--vk-shadow-04),0_14px_40px_var(--vk-shadow-05)]">
          <div className="flex flex-col gap-3.5 p-[22px]">
            <ChoiceCard
              selected={choice === "cloud"}
              onSelect={() => setChoice("cloud")}
              icon={<Cloud className="size-[17px]" />}
              title={t("onboarding.cloudTitle")}
              tag={t("onboarding.cloudTag")}
              tagIcon={<Zap className="size-3" />}
              tagTone="blue"
              description={t("onboarding.cloudDescription")}
            />
            <ChoiceCard
              selected={choice === "local"}
              onSelect={() => setChoice("local")}
              icon={<Laptop className="size-[17px]" />}
              title={t("onboarding.localTitle")}
              tag={t("onboarding.localTag")}
              tagIcon={<Lock className="size-3" />}
              tagTone="green"
              description={t("onboarding.localDescription")}
            />
          </div>
        </div>
      }
      action={
        <PrimaryButton onClick={onContinue} disabled={!choice}>
          {t("onboarding.continue")}
        </PrimaryButton>
      }
    />
  );
}

// ── Step 2 (local only): pick the on-device engine ──────────────────────────
// Its own screen so the 2×2 has room. Privacy is stated once here (the lead),
// not per card. The recommended engine starts highlighted; the user can switch.
// Apple-silicon-only engines are gated on Intel.
function EnginePickStep({
  t,
  detecting,
  appleSiliconDisabled,
  selectedProvider,
  onPickEngine,
  onContinue,
}: {
  t: TFn;
  detecting: boolean;
  appleSiliconDisabled: boolean;
  selectedProvider: Provider;
  onPickEngine: (e: OnDeviceEngine) => void;
  onContinue: () => void;
}) {
  return (
    <StepLayout
      lead={
        <>
          <Title>{t("onboarding.engineScreenTitle")}</Title>
          <Desc>
            {detecting
              ? t("onboarding.localEngineDetecting")
              : t("onboarding.engineScreenSubtitle")}
          </Desc>
        </>
      }
      card={
        <div className="grid auto-rows-fr grid-cols-2 gap-3">
          {ON_DEVICE.map((spec) => (
            <EngineTile
              key={spec.provider}
              t={t}
              spec={spec}
              selected={selectedProvider === spec.provider}
              disabled={spec.requiresAppleSilicon && appleSiliconDisabled}
              onSelect={() => onPickEngine(spec)}
            />
          ))}
        </div>
      }
      action={
        <PrimaryButton onClick={onContinue}>
          {t("onboarding.continue")}
        </PrimaryButton>
      }
    />
  );
}

// One on-device engine in the 2×2 grid. Text-only (no icons), with a solid,
// uniform-height card: name + one strength pill up top, a plain "best for"
// sentence, and a footer (download size · free) pinned to the bottom so all four
// align. `auto-rows-fr` on the grid keeps every card the same height. Pure
// selection — the download happens later in DownloadStep. Copy comes from the
// shared engine catalog's i18n keys, so it matches the Settings picker.
function EngineTile({
  t,
  spec,
  selected,
  disabled,
  onSelect,
}: {
  t: TFn;
  spec: OnDeviceEngine;
  selected: boolean;
  disabled: boolean;
  onSelect: () => void;
}) {
  const pill = spec.pills[0];
  return (
    <button
      type="button"
      onClick={disabled ? undefined : onSelect}
      disabled={disabled}
      aria-pressed={selected}
      className={cn(
        "flex min-h-[138px] flex-col rounded-[16px] border-[1.5px] p-[18px] text-left transition-[border-color,background-color,box-shadow] duration-150",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--vk-accent)] focus-visible:ring-offset-2 focus-visible:ring-offset-[var(--vk-canvas-3)]",
        selected
          ? "border-[var(--vk-accent)] bg-[var(--vk-accent-soft-bg-3)] shadow-[0_0_0_3px_oklch(0.55_0.2_268_/_0.12),0_8px_22px_oklch(0.55_0.2_268_/_0.10)]"
          : "border-[var(--vk-border-strong)] bg-[var(--vk-surface)] shadow-[0_1px_2px_var(--vk-shadow-04),0_4px_14px_var(--vk-shadow-04)] hover:border-[var(--vk-accent)] hover:shadow-[0_4px_16px_var(--vk-shadow-06)]",
        disabled &&
          "cursor-not-allowed border-[var(--vk-border)] opacity-55 shadow-none hover:border-[var(--vk-border)] hover:shadow-none",
      )}
    >
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <span className="text-[15.5px] font-semibold tracking-[-0.01em] text-[var(--vk-text)]">
            {spec.name}
          </span>
          {pill && !disabled ? <TilePill t={t} pill={pill} /> : null}
        </div>
        {selected && !disabled ? (
          <span className="mt-0.5 inline-flex size-[19px] shrink-0 items-center justify-center rounded-full bg-[var(--vk-accent)] text-white shadow-[0_1px_3px_oklch(0.55_0.2_268_/_0.4)]">
            <Check className="size-[13px]" strokeWidth={2.8} />
          </span>
        ) : null}
      </div>
      <p className="mt-2.5 text-[13px] leading-[1.5] text-[var(--vk-text-4)]">
        {t(spec.bestForKey)}
      </p>
      <div
        className={cn(
          "mt-auto border-t pt-3 text-[12px] font-medium",
          selected
            ? "border-[oklch(0.55_0.2_268_/_0.20)] text-[var(--vk-text-6)]"
            : "border-[var(--vk-border)] text-[var(--vk-text-7)]",
        )}
      >
        {disabled ? t("home.enginePicker.needsAppleSilicon") : t(spec.sizeKey)}
      </div>
    </button>
  );
}

function TilePill({ t, pill }: { t: TFn; pill: Pill }) {
  const tone =
    pill.tone === "accent"
      ? "bg-[var(--vk-accent-soft-bg)] text-[var(--vk-accent-2)]"
      : pill.tone === "amber"
        ? "bg-[oklch(0.95_0.05_80)] text-[oklch(0.5_0.13_70)]"
        : "bg-[var(--vk-surface-3)] text-[var(--vk-text-7)]";
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-full px-1.5 py-0.5 text-[10px] font-semibold leading-none tracking-tight",
        tone,
      )}
    >
      {t(pill.key)}
    </span>
  );
}

function ChoiceCard({
  selected,
  onSelect,
  icon,
  title,
  tag,
  tagIcon,
  tagTone,
  description,
}: {
  selected: boolean;
  onSelect: () => void;
  icon: React.ReactNode;
  title: string;
  tag: string;
  tagIcon: React.ReactNode;
  tagTone: "blue" | "green";
  description: string;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      aria-pressed={selected}
      className={cn(
        "rounded-[16px] border-[1.5px] p-[18px] text-left transition-all",
        selected
          ? "border-[var(--vk-accent)] bg-[var(--vk-accent-soft-bg-3)] shadow-[0_0_0_3px_oklch(0.55_0.2_268_/_0.12)]"
          : "border-[var(--vk-border)] bg-[var(--vk-surface)] hover:border-[var(--vk-border-strong)]",
      )}
    >
      <div className="flex items-center gap-2.5">
        <span
          className={cn(
            "grid size-[30px] place-items-center rounded-[9px]",
            selected
              ? "bg-[var(--vk-accent)] text-white"
              : "bg-[var(--vk-surface-3)] text-[var(--vk-text-4)]",
          )}
        >
          {icon}
        </span>
        <span className="text-[15px] font-semibold text-[var(--vk-text)]">
          {title}
        </span>
        <span
          className={cn(
            "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10.5px] font-semibold tracking-[0.02em]",
            tagTone === "blue"
              ? "bg-[var(--vk-info-soft-bg)] text-[var(--vk-info-fg)]"
              : "bg-[var(--vk-success-soft-bg)] text-[var(--vk-success-2)]",
          )}
        >
          {tagIcon}
          {tag}
        </span>
      </div>
      <p className="mt-2.5 text-[12.5px] leading-[1.5] text-[var(--vk-text-7)]">
        {description}
      </p>
    </button>
  );
}

// ── Step 2: download model (local only) ─────────────────────────────────────
function DownloadStep({
  t,
  engine,
  engineShort,
  onNext,
}: {
  t: TFn;
  engine: Engine;
  engineShort: string;
  onNext: () => void;
}) {
  const preparing = useIsPreparing(engine);
  const [ready, setReady] = useState(false);
  const [started, setStarted] = useState(false);
  const [pct, setPct] = useState<number | null>(null);

  // Rehydrate: model already installed → skip straight to ready.
  useEffect(() => {
    let cancelled = false;
    if (!isTauri()) {
      setReady(true);
      return;
    }
    void getModelStatus({ engine })
      .then((s) => {
        if (!cancelled && s.exists) setReady(true);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [engine]);

  // Progress + completion stream (Whisper reports a real %, Parakeet is
  // indeterminate — a single (0, null) tick then complete).
  useEffect(() => {
    let offP: (() => void) | undefined;
    let offC: (() => void) | undefined;
    void subscribeModelProgress((p) => {
      if (p.engine !== engine) return;
      setPct(p.total ? Math.min(100, Math.floor((p.downloaded / p.total) * 100)) : null);
    }).then((o) => (offP = o));
    void subscribeModelComplete((c) => {
      if (c.engine !== engine) return;
      setPct(100);
      setReady(true);
    }).then((o) => (offC = o));
    return () => {
      offP?.();
      offC?.();
    };
  }, [engine]);

  const busy = (started || preparing) && !ready;

  const start = useCallback(() => {
    setStarted(true);
    setPct(null);
    void downloadModel({ engine }).catch(() => setStarted(false));
  }, [engine]);

  return (
    <StepLayout
      lead={
        <>
          <Title>{t("onboarding.dlTitle")}</Title>
          <Desc>{t("onboarding.dlDesc")}</Desc>
        </>
      }
      card={
        <div className="overflow-hidden rounded-[20px] border border-[var(--vk-border)] bg-[var(--vk-surface)] shadow-[0_1px_2px_var(--vk-shadow-04),0_14px_40px_var(--vk-shadow-05)]">
          <div className="p-[26px]">
            <div className="mb-[22px] flex flex-col items-center gap-2.5 rounded-[14px] bg-[var(--vk-accent-soft-bg-3)] px-5 py-[30px]">
              <BrandChip size="md" />
              <div className="bg-gradient-to-r from-[var(--vk-accent-3)] to-[var(--vk-accent-pop-2)] bg-clip-text text-[30px] font-[680] tracking-[-0.02em] text-transparent">
                {engineShort}
              </div>
            </div>

            {busy || ready ? (
              <>
                <div className="text-[18px] font-[650] tracking-[-0.01em] text-[var(--vk-text)]">
                  {ready
                    ? t("onboarding.dlReadyTitle")
                    : t("onboarding.dlProgressTitle")}
                </div>
                <ProgressBar
                  value={ready ? 100 : (pct ?? 8)}
                  gradient
                  className="my-3.5 h-1.5"
                />
                <div className="text-[12.5px] leading-[1.5] text-[var(--vk-text-7)]">
                  {ready
                    ? t("onboarding.dlInstalled")
                    : pct === null
                      ? t("onboarding.dlKeepOpen")
                      : `${pct}% · ${t("onboarding.dlKeepOpen")}`}
                </div>
              </>
            ) : (
              <>
                <div className="text-[18px] font-[650] tracking-[-0.01em] text-[var(--vk-text)]">
                  {t("onboarding.dlExplainTitle")}
                </div>
                <p className="mt-2 text-[12.5px] leading-[1.5] text-[var(--vk-text-7)]">
                  {t("onboarding.dlExplainBody")}
                </p>
              </>
            )}
          </div>
        </div>
      }
      action={
        ready ? (
          <PrimaryButton onClick={onNext}>{t("onboarding.next")}</PrimaryButton>
        ) : busy ? (
          <PrimaryButton onClick={() => {}} disabled>
            <span className="inline-flex items-center gap-2">
              <Loader2 className="size-4 animate-spin motion-reduce:hidden" />
              {t("onboarding.dlDownloading")}
            </span>
          </PrimaryButton>
        ) : (
          <PrimaryButton onClick={start}>
            {t("onboarding.dlDownload")}
          </PrimaryButton>
        )
      }
    />
  );
}

// ── Step 3: grant permissions ───────────────────────────────────────────────
function PermissionsStep({ t, onNext }: { t: TFn; onNext: () => void }) {
  const [status, setStatus] = useState<PermissionsStatus | null>(null);

  const refresh = useCallback(() => {
    if (!isTauri()) {
      // Browser dev: no native permissions — treat as all granted so the
      // flow is walkable.
      setStatus({ accessibility: true, input_monitoring: true, microphone: true });
      return;
    }
    void checkPermissions()
      .then(setStatus)
      .catch(() => {});
  }, []);

  // Re-check on mount and whenever the user returns from System Settings.
  useEffect(() => {
    refresh();
    const onFocus = () => refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  const rows: { pane: SettingsPane; title: string; why: string; granted: boolean }[] =
    [
      {
        pane: "microphone",
        title: t("permissions.microphoneTitle"),
        why: t("permissions.microphoneWhy"),
        granted: !!status?.microphone,
      },
      {
        pane: "accessibility",
        title: t("permissions.accessibilityTitle"),
        why: t("permissions.accessibilityWhy"),
        granted: !!status?.accessibility,
      },
      {
        pane: "input-monitoring",
        title: t("permissions.inputMonitoringTitle"),
        why: t("permissions.inputMonitoringWhy"),
        granted: !!status?.input_monitoring,
      },
    ];
  const allGranted = rows.every((r) => r.granted);

  return (
    <StepLayout
      lead={
        <>
          <div className="mb-[22px]">
            <BrandChip size="md" />
          </div>
          <Title>{t("onboarding.permsTitle")}</Title>
          <Desc>{t("onboarding.permsDesc")}</Desc>
        </>
      }
      card={
        <div className="overflow-hidden rounded-[20px] border border-[var(--vk-border)] bg-[var(--vk-surface)] shadow-[0_1px_2px_var(--vk-shadow-04),0_14px_40px_var(--vk-shadow-05)]">
          {rows.map((r) => (
            <div
              key={r.pane}
              className="border-t border-[var(--vk-border-3)] px-[22px] py-5 first:border-t-0"
            >
              <div className="flex items-start justify-between gap-3.5">
                <div>
                  <div className="text-[15px] font-semibold text-[var(--vk-text)]">
                    {r.title}
                  </div>
                  <div className="mt-1.5 max-w-[42ch] text-[12.5px] leading-[1.5] text-[var(--vk-text-7)]">
                    {r.why}
                  </div>
                </div>
                {r.granted ? (
                  <span className="inline-flex shrink-0 items-center gap-1.5 text-[13px] font-semibold text-[var(--vk-success-2)]">
                    <Check className="size-3.5" strokeWidth={2.6} />
                    {t("permissions.statusGranted")}
                  </span>
                ) : (
                  <button
                    type="button"
                    onClick={() => void openSettingsPane(r.pane)}
                    className="shrink-0 whitespace-nowrap rounded-lg border border-[var(--vk-border-strong)] bg-[var(--vk-surface)] px-[11px] py-1.5 text-[12.5px] font-semibold text-[var(--vk-accent-2)] transition-colors hover:border-[var(--vk-accent)] hover:bg-[var(--vk-accent-soft-bg)]"
                  >
                    {t("permissions.openSettings")}
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      }
      action={
        <PrimaryButton onClick={onNext} disabled={!allGranted}>
          {t("onboarding.next")}
        </PrimaryButton>
      }
    />
  );
}

// ── Step 4: choose microphone (with live meter) ─────────────────────────────
function MicStep({
  t,
  settings,
  update,
  onNext,
}: {
  t: TFn;
  settings: Settings;
  update: (patch: Partial<Settings>) => Promise<void>;
  onNext: () => void;
}) {
  const [devices, setDevices] = useState<string[]>([]);
  // Rolling buffer of recent RMS values → a scrolling VU meter on the selected
  // row. Driven by real audio:level events from the Rust monitor.
  const BARS = 9;
  const levelsRef = useRef<number[]>(new Array(BARS).fill(0));
  const [, setTick] = useState(0);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    void listen<string[]>("audio:input-devices-changed", (event) => {
      if (!cancelled) setDevices(event.payload);
    }).then(async (stop) => {
      if (cancelled) { stop(); return; }
      unlisten = stop;
      const list = await listInputDevices();
      if (!cancelled) setDevices(list);
    }).catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Start the native monitor on mount and RE-TARGET it whenever the device
  // changes. `start_mic_monitor` is idempotent and reopens on a new device, so
  // we never stop-then-start between switches — doing so raced two unordered
  // invokes and could leave monitoring OFF (dead meter). Stop happens only when
  // the whole step unmounts (below), so the mic indicator turns off on exit.
  useEffect(() => {
    if (!isTauri()) return;
    void startMicMonitor(settings.micDevice).catch(() => {});
  }, [settings.micDevice]);

  useEffect(() => {
    if (!isTauri()) return;
    return () => {
      void stopMicMonitor().catch(() => {});
    };
  }, []);

  // Subscribe once; push each RMS into the rolling buffer.
  useEffect(() => {
    if (!isTauri()) return;
    let off: (() => void) | undefined;
    void subscribeAudioLevel(({ rms }) => {
      // Map RMS (~0.02–0.15 for speech) into a 0–1 visual level. A sqrt curve
      // gives quiet speech visible movement while still topping out loud audio.
      const level = Math.min(1, Math.sqrt(Math.max(0, rms) * 4));
      const next = levelsRef.current.slice(1);
      next.push(level);
      levelsRef.current = next;
      setTick((n) => (n + 1) % 1_000_000);
    }).then((o) => (off = o));
    return () => off?.();
  }, []);

  const select = (device: string) => void update({ micDevice: device });

  const options = [
    { value: "", label: t("onboarding.micSystemDefault") },
    ...devices.map((name) => ({ value: name, label: name })),
  ];
  if (settings.micDevice && !devices.includes(settings.micDevice)) {
    options.push({ value: settings.micDevice, label: settings.micDevice });
  }

  return (
    <StepLayout
      lead={
        <>
          <Title>{t("onboarding.micTitle")}</Title>
          <Desc>{t("onboarding.micDesc")}</Desc>
        </>
      }
      card={
        <div className="p-0.5">
          {options.map((o) => {
            const selected = settings.micDevice === o.value;
            return (
              <button
                type="button"
                key={o.value || "__default"}
                onClick={() => select(o.value)}
                className={cn(
                  "mt-2.5 flex w-full items-center justify-between gap-3 rounded-[14px] border-[1.5px] bg-[var(--vk-surface)] px-[18px] py-[15px] text-left transition-all first:mt-0",
                  selected
                    ? "border-[var(--vk-text)] shadow-[0_1px_3px_var(--vk-shadow-08)]"
                    : "border-transparent hover:bg-[var(--vk-surface-2)]",
                )}
              >
                <span className="flex min-w-0 items-center gap-2.5">
                  <span
                    className={cn(
                      "grid size-[22px] shrink-0 place-items-center rounded-full",
                      selected
                        ? "bg-[var(--vk-success-soft-bg)] text-[var(--vk-success-2)]"
                        : "bg-[var(--vk-surface-3)] text-transparent",
                    )}
                  >
                    <Check className="size-3.5" strokeWidth={2.6} />
                  </span>
                  <span className="truncate text-[14px] font-medium text-[var(--vk-text-2)]">
                    {o.label}
                  </span>
                </span>
                <Meter
                  active={selected}
                  levels={selected ? levelsRef.current : null}
                />
              </button>
            );
          })}
        </div>
      }
      action={<PrimaryButton onClick={onNext}>{t("onboarding.next")}</PrimaryButton>}
    />
  );
}

function Meter({
  active,
  levels,
}: {
  active: boolean;
  levels: number[] | null;
}) {
  const bars = levels ?? new Array(9).fill(0);
  return (
    <span className="flex h-[18px] shrink-0 items-center gap-[3px]">
      {bars.map((v, i) => {
        // Center-weighted so the meter reads as a waveform, not a flat block.
        const c = Math.abs(i - (bars.length - 1) / 2);
        const h = active ? Math.max(0.16, v * (1 - c * 0.1)) : 0.2;
        return (
          <span
            key={i}
            className={cn(
              "w-[3px] rounded-full transition-[height] duration-100 ease-out",
              active ? "bg-[var(--vk-accent)]" : "bg-[oklch(0.78_0.05_268)]",
            )}
            style={{ height: `${h * 100}%` }}
          />
        );
      })}
    </span>
  );
}

// ── Step 5: done ────────────────────────────────────────────────────────────
function DoneStep({
  t,
  settings,
  onComplete,
}: {
  t: TFn;
  settings: Settings;
  onComplete: () => void;
}) {
  const keys = formatHotkey(settings.recordHotkey).split(" + ");
  return (
    <StepLayout
      lead={
        <>
          <div className="mb-[22px]">
            <BrandChip size="md" />
          </div>
          <Title>{t("onboarding.doneTitle")}</Title>
          <Desc>{t("onboarding.doneDesc")}</Desc>
        </>
      }
      card={
        <div className="overflow-hidden rounded-[20px] border border-[var(--vk-border)] bg-[var(--vk-surface)] shadow-[0_1px_2px_var(--vk-shadow-04),0_14px_40px_var(--vk-shadow-05)]">
          <div className="p-[26px]">
            <div className="mb-2.5 text-[12.5px] font-semibold text-[var(--vk-text-4)]">
              {t("onboarding.doneHotkeyLabel")}
            </div>
            <div className="flex gap-1.5">
              {keys.map((k, i) => (
                <span
                  key={i}
                  className="grid h-[34px] min-w-[34px] place-items-center rounded-[9px] border border-[var(--vk-border-strong)] bg-[var(--vk-surface)] px-2.5 text-[14px] font-semibold text-[var(--vk-text-2)] shadow-[0_2px_0_var(--vk-border-strong)]"
                >
                  {k}
                </span>
              ))}
            </div>
            <div className="mt-[18px] space-y-2.5">
              <Tip>{t("onboarding.doneTip1")}</Tip>
              <Tip>{t("onboarding.doneTip2")}</Tip>
            </div>
          </div>
        </div>
      }
      action={
        <PrimaryButton onClick={onComplete}>
          {t("onboarding.doneStart")}
        </PrimaryButton>
      }
    />
  );
}

function Tip({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex items-start gap-2.5 text-[13px] leading-[1.5] text-[var(--vk-text-7)]">
      <span className="mt-[7px] size-[5px] shrink-0 rounded-full bg-[var(--vk-accent)]" />
      <div>{children}</div>
    </div>
  );
}
