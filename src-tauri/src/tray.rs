//! macOS menu-bar tray icon.
//!
//! Items: Show Vibeking, STT Language radio group, STT Provider radio
//! group, Quit. Selections update the in-memory settings cache
//! immediately and emit events so the frontend can persist + re-render.
//!
//! Language list is filtered per-provider — Parakeet only supports its
//! 25 European languages, so the curated top-10 set drops the CJK
//! entries when fluid-audio is active. Labels are localized off the
//! user's `settings.language` (zh-CN / zh-TW / en).

use std::collections::HashMap;
use std::sync::Mutex;

use tauri::{
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Runtime, Wry,
};

use crate::state::SharedState;
use crate::stt::Provider;

const TRAY_ID: &str = "vibeking-tray";
const SHOW_MAIN: &str = "main:show";
const QUIT: &str = "app:quit";
const PERSISTENT_BAR: &str = "persistent-bar";
/// Manual "rescue" for the floating bar when macOS strands it off-screen after a
/// display change — recreates the chip window (the only reliable recovery).
const RESTORE_BAR: &str = "restore-bar";

const LANG_PREFIX: &str = "lang:";
const PROVIDER_PREFIX: &str = "provider:";
const REFINEMENT_PREFIX: &str = "refinement:";
const MIC_PREFIX: &str = "mic:";

const PICK_LANGUAGES: &str = "pick-languages";
/// Sentinel id for the "Off" entry in the refinement-mode submenu — keeps
/// the click handler simple (string-match on the suffix) instead of
/// special-casing an `Option<String>` payload.
const REFINEMENT_OFF: &str = "__off__";
/// Sentinel id for the "System default" entry in the microphone submenu.
/// Maps to an empty `settings.mic_device`, matching the existing JSON
/// shape — no migration needed.
const MIC_SYSTEM_DEFAULT: &str = "__system_default__";

/// Full Whisper-99 label table. Must stay in sync with
/// `src/lib/stt-languages.ts::LABELS`. Pulled into the tray so
/// favorites outside the legacy top-10 still get a readable label
/// (e.g. the user stars Vietnamese — they should see "Tiếng Việt ·
/// Vietnamese" in the menu, not "vi").
const LANGUAGE_LABELS: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("am", "አማርኛ · Amharic"),
    ("ar", "العربية · Arabic"),
    ("as", "অসমীয়া · Assamese"),
    ("az", "Azərbaycanca · Azerbaijani"),
    ("ba", "Башҡортса · Bashkir"),
    ("be", "Беларуская · Belarusian"),
    ("bg", "Български · Bulgarian"),
    ("bn", "বাংলা · Bengali"),
    ("bo", "བོད་སྐད་ · Tibetan"),
    ("br", "Brezhoneg · Breton"),
    ("bs", "Bosanski · Bosnian"),
    ("ca", "Català · Catalan"),
    ("cs", "Čeština · Czech"),
    ("cy", "Cymraeg · Welsh"),
    ("da", "Dansk · Danish"),
    ("de", "Deutsch · German"),
    ("el", "Ελληνικά · Greek"),
    ("en", "English"),
    ("es", "Español · Spanish"),
    ("et", "Eesti · Estonian"),
    ("eu", "Euskara · Basque"),
    ("fa", "فارسی · Persian"),
    ("fi", "Suomi · Finnish"),
    ("fo", "Føroyskt · Faroese"),
    ("fr", "Français · French"),
    ("gl", "Galego · Galician"),
    ("gu", "ગુજરાતી · Gujarati"),
    ("ha", "Hausa"),
    ("haw", "ʻŌlelo Hawaiʻi · Hawaiian"),
    ("he", "עברית · Hebrew"),
    ("hi", "हिन्दी · Hindi"),
    ("hr", "Hrvatski · Croatian"),
    ("ht", "Kreyòl Ayisyen · Haitian Creole"),
    ("hu", "Magyar · Hungarian"),
    ("hy", "Հայերեն · Armenian"),
    ("id", "Bahasa Indonesia"),
    ("is", "Íslenska · Icelandic"),
    ("it", "Italiano · Italian"),
    ("ja", "日本語 · Japanese"),
    ("jw", "Basa Jawa · Javanese"),
    ("ka", "ქართული · Georgian"),
    ("kk", "Қазақша · Kazakh"),
    ("km", "ខ្មែរ · Khmer"),
    ("kn", "ಕನ್ನಡ · Kannada"),
    ("ko", "한국어 · Korean"),
    ("la", "Latina · Latin"),
    ("lb", "Lëtzebuergesch · Luxembourgish"),
    ("ln", "Lingála · Lingala"),
    ("lo", "ລາວ · Lao"),
    ("lt", "Lietuvių · Lithuanian"),
    ("lv", "Latviešu · Latvian"),
    ("mg", "Malagasy"),
    ("mi", "Te Reo Māori · Maori"),
    ("mk", "Македонски · Macedonian"),
    ("ml", "മലയാളം · Malayalam"),
    ("mn", "Монгол · Mongolian"),
    ("mr", "मराठी · Marathi"),
    ("ms", "Bahasa Melayu · Malay"),
    ("mt", "Malti · Maltese"),
    ("my", "မြန်မာ · Burmese"),
    ("ne", "नेपाली · Nepali"),
    ("nl", "Nederlands · Dutch"),
    ("nn", "Nynorsk · Norwegian Nynorsk"),
    ("no", "Norsk · Norwegian"),
    ("oc", "Occitan"),
    ("pa", "ਪੰਜਾਬੀ · Punjabi"),
    ("pl", "Polski · Polish"),
    ("ps", "پښتو · Pashto"),
    ("pt", "Português · Portuguese"),
    ("ro", "Română · Romanian"),
    ("ru", "Русский · Russian"),
    ("sa", "संस्कृतम् · Sanskrit"),
    ("sd", "سنڌي · Sindhi"),
    ("si", "සිංහල · Sinhala"),
    ("sk", "Slovenčina · Slovak"),
    ("sl", "Slovenščina · Slovenian"),
    ("sn", "ChiShona · Shona"),
    ("so", "Soomaali · Somali"),
    ("sq", "Shqip · Albanian"),
    ("sr", "Српски · Serbian"),
    ("su", "Basa Sunda · Sundanese"),
    ("sv", "Svenska · Swedish"),
    ("sw", "Kiswahili · Swahili"),
    ("ta", "தமிழ் · Tamil"),
    ("te", "తెలుగు · Telugu"),
    ("tg", "Тоҷикӣ · Tajik"),
    ("th", "ไทย · Thai"),
    ("tk", "Türkmençe · Turkmen"),
    ("tl", "Tagalog"),
    ("tr", "Türkçe · Turkish"),
    ("tt", "Татарча · Tatar"),
    ("uk", "Українська · Ukrainian"),
    ("ur", "اردو · Urdu"),
    ("uz", "Oʻzbekcha · Uzbek"),
    ("vi", "Tiếng Việt · Vietnamese"),
    ("yi", "ייִדיש · Yiddish"),
    ("yo", "Yorùbá · Yoruba"),
    ("yue", "粵語 · Cantonese"),
    ("zh", "中文 · Chinese"),
];

fn language_label(code: &str) -> String {
    LANGUAGE_LABELS
        .iter()
        .find_map(|(k, v)| {
            if *k == code {
                Some((*v).to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| code.to_string())
}

fn language_known(code: &str) -> bool {
    LANGUAGE_LABELS.iter().any(|(k, _)| *k == code)
}

/// Parakeet TDT v3 ISO 639-1 codes — must stay in sync with
/// `src/lib/stt-languages.ts::PARAKEET_25`. The intersection with
/// `LANGUAGES` after `"auto"` is: en, es, fr, de, pt, ru. The CJK
/// entries (zh / ja / ko) drop off.
const PARAKEET_LANGS: &[&str] = &[
    "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv", "lt", "mt",
    "pl", "pt", "ro", "ru", "sk", "sl", "es", "sv", "uk",
];

fn parakeet_supports(code: &str) -> bool {
    code == "auto" || PARAKEET_LANGS.contains(&code)
}

/// Qwen3-ASR languages — must stay in sync with `stt-languages.ts::QWEN3_LANGS`.
/// Multilingual with first-class CJK (zh, yue, ja, ko) plus ~26 others.
const QWEN3_LANGS: &[&str] = &[
    "zh", "yue", "en", "ja", "ko", "vi", "th", "id", "ms", "ar", "hi", "fa", "tr", "ru", "de",
    "fr", "es", "pt", "it", "nl", "sv", "da", "fi", "pl", "cs", "el", "hu", "ro", "mk", "tl",
];

fn qwen3_supports(code: &str) -> bool {
    code == "auto" || QWEN3_LANGS.contains(&code)
}

/// Gemma 4 audio languages — must stay in sync with `stt-languages.ts::GEMMA_LANGS`.
/// Gemma's multimodal coverage (~35) is narrower than its text set.
const GEMMA_LANGS: &[&str] = &[
    "en", "zh", "ja", "ko", "de", "fr", "es", "pt", "it", "nl", "ru", "ar", "hi", "id", "vi", "th",
    "tr", "pl", "uk", "cs", "el", "hu", "ro", "sv", "da", "fi", "he", "fa", "ta", "te", "bn", "ml",
    "sw",
];

fn gemma_supports(code: &str) -> bool {
    code == "auto" || GEMMA_LANGS.contains(&code)
}

/// Full provider menu set. Filtered by [`available_providers`] at runtime
/// so the Parakeet entry is hidden on Intel Macs (where the model fails
/// to load).
// Plain product voice — brand name only, no model IDs / accelerator jargon
// (PRODUCT.md), symmetric Cloud · / Local · prefixes.
const PROVIDERS: &[(&str, &str)] = &[
    ("deepgram", "Cloud · Deepgram"),
    ("groq", "Cloud · Groq"),
    ("openai", "Cloud · OpenAI"),
    ("elevenlabs", "Cloud · ElevenLabs"),
    ("local", "Local · Whisper"),
    ("fluid-audio", "Local · Parakeet"),
    ("qwen3", "Local · Qwen3"),
    ("gemma", "Local · Gemma"),
];

/// Apple-Silicon-only providers — hidden on Intel.
const APPLE_SILICON_ONLY_PROVIDERS: &[&str] = &["fluid-audio", "qwen3", "gemma"];

fn available_providers() -> Vec<(&'static str, &'static str)> {
    let is_apple_silicon = std::env::consts::ARCH == "aarch64";
    PROVIDERS
        .iter()
        .copied()
        .filter(|(id, _)| is_apple_silicon || !APPLE_SILICON_ONLY_PROVIDERS.contains(id))
        .collect()
}

/// Localized strings for the tray. Brand/provider labels stay in their
/// original form since they're product names; only the user-facing
/// English chrome gets translated.
struct TrayStrings {
    show: &'static str,
    quit: &'static str,
    language: &'static str,
    provider: &'static str,
    auto_detect: &'static str,
    pick_languages: &'static str,
    refinement: &'static str,
    refinement_off: &'static str,
    microphone: &'static str,
    mic_system_default: &'static str,
    mic_unavailable_suffix: &'static str,
    persistent_bar: &'static str,
    restore_bar: &'static str,
}

fn tray_strings(ui_lang: &str) -> TrayStrings {
    let normalized = match ui_lang {
        "zh-CN" | "zh" | "zh-Hans" => "zh-CN",
        "zh-TW" | "zh-Hant" => "zh-TW",
        _ => "en",
    };
    match normalized {
        "zh-CN" => TrayStrings {
            show: "显示 Vibeking",
            quit: "退出 Vibeking",
            language: "识别语言",
            provider: "识别引擎",
            auto_detect: "自动检测",
            pick_languages: "选择语言…",
            refinement: "润色模式",
            refinement_off: "关闭",
            microphone: "麦克风",
            mic_system_default: "系统默认",
            mic_unavailable_suffix: "(未连接)",
            persistent_bar: "常驻悬浮条",
            restore_bar: "恢复悬浮条",
        },
        "zh-TW" => TrayStrings {
            show: "顯示 Vibeking",
            quit: "結束 Vibeking",
            language: "辨識語言",
            provider: "辨識引擎",
            auto_detect: "自動偵測",
            pick_languages: "選擇語言…",
            refinement: "潤色模式",
            refinement_off: "關閉",
            microphone: "麥克風",
            mic_system_default: "系統預設",
            mic_unavailable_suffix: "(未連接)",
            persistent_bar: "常駐懸浮列",
            restore_bar: "恢復懸浮列",
        },
        _ => TrayStrings {
            show: "Show Vibeking",
            quit: "Quit Vibeking",
            language: "STT Language",
            provider: "STT Provider",
            auto_detect: "Auto-detect",
            pick_languages: "Pick languages…",
            refinement: "Refinement",
            refinement_off: "Off",
            microphone: "Microphone",
            mic_system_default: "System default",
            mic_unavailable_suffix: "(not connected)",
            persistent_bar: "Always-on bar",
            restore_bar: "Restore floating bar",
        },
    }
}

static LANG_ITEMS: Mutex<Option<HashMap<String, CheckMenuItem<Wry>>>> = Mutex::new(None);
static PROVIDER_ITEMS: Mutex<Option<HashMap<String, CheckMenuItem<Wry>>>> = Mutex::new(None);
/// Refinement-mode check items keyed by id. The "Off" entry uses
/// `REFINEMENT_OFF` as its key.
static REFINEMENT_ITEMS: Mutex<Option<HashMap<String, CheckMenuItem<Wry>>>> = Mutex::new(None);
/// Microphone check items keyed by device name. The "System default"
/// entry uses `MIC_SYSTEM_DEFAULT` as its key.
static MIC_ITEMS: Mutex<Option<HashMap<String, CheckMenuItem<Wry>>>> = Mutex::new(None);
/// Signature of the refinement state used to decide if the menu needs a
/// full rebuild. `(active_id, [(id, name, emoji), ...])` — captures name +
/// emoji so renaming a mode triggers a relabel.
type RefinementKey = (Option<String>, Vec<(String, String, String)>);
/// Signature of the microphone state. `(saved_device, [available_device_names...])`.
/// Device list is re-enumerated on every rebuild — when a USB mic gets
/// plugged in between settings:changed events, the user won't see it
/// until the next rebuild fires. Acceptable for v1.
type MicKey = (String, Vec<String>);
/// Last (ui_language, provider_id, favorites, refinement_key, mic_key)
/// the tray was built against. Used by `maybe_rebuild` to skip no-op
/// rebuilds when an unrelated setting changes.
static LAST_TRAY_KEY: Mutex<Option<(String, String, Vec<String>, RefinementKey, MicKey, bool)>> =
    Mutex::new(None);

pub fn setup(app: &AppHandle<Wry>) -> tauri::Result<TrayIcon<Wry>> {
    let menu = build_and_store_menu(app)?;

    // Monochrome template glyph rendered from src/assets/vibeking-menubar.svg.
    // `icon_as_template(true)` lets macOS auto-tint it for light/dark menu bars.
    const MENUBAR_PNG: &[u8] = include_bytes!("../icons/menubar-template.png");
    let icon = tauri::image::Image::from_bytes(MENUBAR_PNG)
        .map_err(|e| tauri::Error::Anyhow(anyhow::anyhow!("load menubar icon: {e}")))?;

    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Vibeking")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::DoubleClick { .. } = event {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    crate::audio::refresh_input_devices(app);
    Ok(tray)
}

/// Rebuild the tray menu only if something that actually changes the
/// menu shape or labels has changed. Frontend's `settings:changed`
/// fires for any settings mutation (hotwords, dictionary, etc.); we
/// don't want to rebuild for those.
pub fn maybe_rebuild(app: &AppHandle<Wry>) {
    let key = current_tray_key(app);
    {
        let Ok(guard) = LAST_TRAY_KEY.lock() else {
            return;
        };
        if guard.as_ref() == Some(&key) {
            return;
        }
    }
    rebuild(app);
}

/// Force-rebuild the tray menu against current settings. Used by the
/// in-tray provider toggle (where we know the menu's stale) and as the
/// fallback from `maybe_rebuild`.
fn rebuild(app: &AppHandle<Wry>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let Ok(menu) = build_and_store_menu(app) else {
        return;
    };
    if let Err(e) = tray.set_menu(Some(menu)) {
        log::error!("[vibeking] tray set_menu failed: {e}");
    }
}

fn current_tray_key(
    app: &AppHandle<Wry>,
) -> (String, String, Vec<String>, RefinementKey, MicKey, bool) {
    let state = app.state::<SharedState>();
    let s = state.settings.lock();
    let refinement_key: RefinementKey = (
        s.active_refinement_mode_id.clone(),
        s.refinement_modes
            .iter()
            .map(|m| (m.id.clone(), m.name.clone(), m.emoji.clone()))
            .collect(),
    );
    let mic_key: MicKey = (s.mic_device.clone(), crate::audio::cached_input_devices());
    (
        s.language.clone(),
        provider_id(s.provider),
        s.favorite_stt_languages.clone(),
        refinement_key,
        mic_key,
        s.persistent_bar,
    )
}

fn build_and_store_menu(app: &AppHandle<Wry>) -> tauri::Result<Menu<Wry>> {
    let (
        current_lang,
        current_provider,
        ui_lang,
        favorites,
        refinement_modes,
        active_refinement_id,
        mic_device,
        persistent_bar,
    ) = {
        let state = app.state::<SharedState>();
        let s = state.settings.lock();
        (
            s.stt_language.clone(),
            provider_id(s.provider),
            s.language.clone(),
            s.favorite_stt_languages.clone(),
            s.refinement_modes.clone(),
            s.active_refinement_mode_id.clone(),
            s.mic_device.clone(),
            s.persistent_bar,
        )
    };
    let strings = tray_strings(&ui_lang);
    let available_mics = crate::audio::cached_input_devices();

    let show_main = MenuItem::with_id(app, SHOW_MAIN, strings.show, true, None::<&str>)?;

    let lang_options = languages_for_provider(&current_provider, &favorites, &strings);
    let needs_pick_languages = lang_options.len() <= 1;
    let (language_submenu, lang_map) = build_language_submenu(
        app,
        strings.language,
        &lang_options,
        &current_lang,
        needs_pick_languages.then_some(strings.pick_languages),
    )?;
    let providers = available_providers();
    let (provider_submenu, provider_map) = build_radio_submenu(
        app,
        strings.provider,
        &providers,
        PROVIDER_PREFIX,
        &current_provider,
    )?;

    let (refinement_submenu, refinement_map) = build_refinement_submenu(
        app,
        strings.refinement,
        strings.refinement_off,
        &refinement_modes,
        active_refinement_id.as_deref(),
    )?;

    let (mic_submenu, mic_map) = build_microphone_submenu(
        app,
        strings.microphone,
        strings.mic_system_default,
        strings.mic_unavailable_suffix,
        &available_mics,
        &mic_device,
    )?;

    let persistent_bar_item = CheckMenuItem::with_id(
        app,
        PERSISTENT_BAR,
        strings.persistent_bar,
        true,
        persistent_bar,
        None::<&str>,
    )?;

    let restore_bar_item =
        MenuItem::with_id(app, RESTORE_BAR, strings.restore_bar, true, None::<&str>)?;

    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, QUIT, strings.quit, true, None::<&str>)?;

    let items: [&dyn IsMenuItem<Wry>; 11] = [
        &show_main,
        &separator,
        &refinement_submenu,
        &mic_submenu,
        &language_submenu,
        &provider_submenu,
        &separator,
        &persistent_bar_item,
        &restore_bar_item,
        &separator,
        &quit,
    ];
    let menu = Menu::with_items(app, &items)?;

    if let Ok(mut slot) = LANG_ITEMS.lock() {
        *slot = Some(lang_map);
    }
    if let Ok(mut slot) = PROVIDER_ITEMS.lock() {
        *slot = Some(provider_map);
    }
    if let Ok(mut slot) = REFINEMENT_ITEMS.lock() {
        *slot = Some(refinement_map);
    }
    if let Ok(mut slot) = MIC_ITEMS.lock() {
        *slot = Some(mic_map);
    }
    if let Ok(mut slot) = LAST_TRAY_KEY.lock() {
        let refinement_key: RefinementKey = (
            active_refinement_id,
            refinement_modes
                .iter()
                .map(|m| (m.id.clone(), m.name.clone(), m.emoji.clone()))
                .collect(),
        );
        let mic_key: MicKey = (mic_device, available_mics);
        *slot = Some((
            ui_lang,
            current_provider,
            favorites,
            refinement_key,
            mic_key,
            persistent_bar,
        ));
    }

    Ok(menu)
}

/// Microphone submenu: "System default" at the top, then every cpal
/// input device. When the saved device isn't currently in the device
/// list (e.g. AirPods disconnected), append it at the bottom with the
/// localized "(not connected)" suffix so the user sees their saved
/// preference and knows it'll route to the system default for now.
fn build_microphone_submenu(
    app: &AppHandle<Wry>,
    title: &str,
    system_default_label: &str,
    unavailable_suffix: &str,
    available: &[String],
    saved: &str,
) -> tauri::Result<(Submenu<Wry>, HashMap<String, CheckMenuItem<Wry>>)> {
    let saved_trim = saved.trim();
    let default_active = saved_trim.is_empty();

    let default_item = CheckMenuItem::with_id(
        app,
        format!("{MIC_PREFIX}{MIC_SYSTEM_DEFAULT}"),
        system_default_label,
        true,
        default_active,
        None::<&str>,
    )?;

    let mut device_items: Vec<(String, CheckMenuItem<Wry>)> =
        Vec::with_capacity(available.len() + 1);
    for name in available {
        let item = CheckMenuItem::with_id(
            app,
            format!("{MIC_PREFIX}{name}"),
            name.as_str(),
            true,
            !default_active && name == saved_trim,
            None::<&str>,
        )?;
        device_items.push((name.clone(), item));
    }

    // Saved-but-not-connected: surface it so the user can see their
    // preference is still set even though it's offline. Selectable so
    // the user can keep it set when temporarily disconnected — the
    // recording path falls back to system default until it reappears.
    let saved_missing = !default_active && !available.iter().any(|n| n == saved_trim);
    if saved_missing {
        let label = format!("{saved_trim} {unavailable_suffix}");
        let item = CheckMenuItem::with_id(
            app,
            format!("{MIC_PREFIX}{saved_trim}"),
            label,
            true,
            true,
            None::<&str>,
        )?;
        device_items.push((saved_trim.to_string(), item));
    }

    let separator = PredefinedMenuItem::separator(app)?;
    let mut refs: Vec<&dyn IsMenuItem<Wry>> = Vec::with_capacity(device_items.len() + 2);
    refs.push(&default_item as &dyn IsMenuItem<Wry>);
    if !device_items.is_empty() {
        refs.push(&separator as &dyn IsMenuItem<Wry>);
        for (_, item) in &device_items {
            refs.push(item as &dyn IsMenuItem<Wry>);
        }
    }
    let submenu = Submenu::with_items(app, title, true, &refs)?;

    let mut map: HashMap<String, CheckMenuItem<Wry>> =
        HashMap::with_capacity(device_items.len() + 1);
    map.insert(MIC_SYSTEM_DEFAULT.to_string(), default_item);
    for (name, item) in device_items {
        map.insert(name, item);
    }

    Ok((submenu, map))
}

/// Refinement-mode submenu: "Off" entry at the top, then every configured
/// mode (built-in or custom). The active one carries the check mark.
/// `refinement_modes` is taken by reference because the caller already
/// cloned it from the settings lock — building the menu shouldn't take
/// the lock again.
fn build_refinement_submenu(
    app: &AppHandle<Wry>,
    title: &str,
    off_label: &str,
    refinement_modes: &[crate::state::RefinementMode],
    active_id: Option<&str>,
) -> tauri::Result<(Submenu<Wry>, HashMap<String, CheckMenuItem<Wry>>)> {
    let off_item = CheckMenuItem::with_id(
        app,
        format!("{REFINEMENT_PREFIX}{REFINEMENT_OFF}"),
        off_label,
        true,
        active_id.is_none(),
        None::<&str>,
    )?;

    let mode_items: Vec<CheckMenuItem<Wry>> = refinement_modes
        .iter()
        .map(|m| {
            CheckMenuItem::with_id(
                app,
                format!("{REFINEMENT_PREFIX}{}", m.id),
                format!("{} {}", m.emoji, m.name),
                true,
                Some(m.id.as_str()) == active_id,
                None::<&str>,
            )
        })
        .collect::<tauri::Result<Vec<_>>>()?;

    let separator = PredefinedMenuItem::separator(app)?;
    let mut refs: Vec<&dyn IsMenuItem<Wry>> = Vec::with_capacity(mode_items.len() + 2);
    refs.push(&off_item as &dyn IsMenuItem<Wry>);
    refs.push(&separator as &dyn IsMenuItem<Wry>);
    for item in &mode_items {
        refs.push(item as &dyn IsMenuItem<Wry>);
    }
    let submenu = Submenu::with_items(app, title, true, &refs)?;

    let mut map: HashMap<String, CheckMenuItem<Wry>> =
        HashMap::with_capacity(refinement_modes.len() + 1);
    map.insert(REFINEMENT_OFF.to_string(), off_item);
    for (mode, item) in refinement_modes.iter().zip(mode_items.into_iter()) {
        map.insert(mode.id.clone(), item);
    }

    Ok((submenu, map))
}

/// Builds the language menu items: `auto` first, then favorites in the
/// order the user starred them, then drops anything the active provider
/// can't actually transcribe. The `auto_detect` label is localized; the
/// other labels come from the master `LANGUAGE_LABELS` table.
fn languages_for_provider(
    provider: &str,
    favorites: &[String],
    strings: &TrayStrings,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(favorites.len() + 1);
    out.push(("auto".to_string(), strings.auto_detect.to_string()));
    for code in favorites {
        if code == "auto" {
            continue; // never store "auto" in favorites, but be defensive
        }
        if !language_known(code) {
            continue; // unknown / stale code — silently skip
        }
        if provider == "fluid-audio" && !parakeet_supports(code) {
            continue;
        }
        if provider == "qwen3" && !qwen3_supports(code) {
            continue;
        }
        if provider == "gemma" && !gemma_supports(code) {
            continue;
        }
        out.push((code.clone(), language_label(code)));
    }
    out
}

/// Language submenu builder. Unlike the generic provider builder it
/// owns its strings (favorites flow through as `String`) and optionally
/// appends a separator + "Pick languages…" tail item when the
/// favorites set is empty for the active provider — opens the main
/// window so the user can pick from the full Combobox list.
fn build_language_submenu(
    app: &AppHandle<Wry>,
    title: &str,
    options: &[(String, String)],
    active: &str,
    pick_languages_label: Option<&str>,
) -> tauri::Result<(Submenu<Wry>, HashMap<String, CheckMenuItem<Wry>>)> {
    let mut items: Vec<CheckMenuItem<Wry>> = Vec::with_capacity(options.len());
    for (value, label) in options {
        let id = format!("{LANG_PREFIX}{value}");
        items.push(CheckMenuItem::with_id(
            app,
            &id,
            label.as_str(),
            true,
            value == active,
            None::<&str>,
        )?);
    }

    let separator = if pick_languages_label.is_some() {
        Some(PredefinedMenuItem::separator(app)?)
    } else {
        None
    };
    let pick_item = if let Some(label) = pick_languages_label {
        Some(MenuItem::with_id(
            app,
            PICK_LANGUAGES,
            label,
            true,
            None::<&str>,
        )?)
    } else {
        None
    };

    let mut refs: Vec<&dyn IsMenuItem<Wry>> =
        items.iter().map(|i| i as &dyn IsMenuItem<Wry>).collect();
    if let Some(sep) = &separator {
        refs.push(sep as &dyn IsMenuItem<Wry>);
    }
    if let Some(pick) = &pick_item {
        refs.push(pick as &dyn IsMenuItem<Wry>);
    }

    let submenu = Submenu::with_items(app, title, true, &refs)?;

    let map: HashMap<String, CheckMenuItem<Wry>> = options
        .iter()
        .zip(items.into_iter())
        .map(|((value, _), item)| (value.clone(), item))
        .collect();

    Ok((submenu, map))
}

fn build_radio_submenu<L: AsRef<str>>(
    app: &AppHandle<Wry>,
    title: &str,
    options: &[(&str, L)],
    prefix: &str,
    active: &str,
) -> tauri::Result<(Submenu<Wry>, HashMap<String, CheckMenuItem<Wry>>)> {
    let mut items: Vec<CheckMenuItem<Wry>> = Vec::with_capacity(options.len());
    for (value, label) in options {
        let id = format!("{prefix}{value}");
        items.push(CheckMenuItem::with_id(
            app,
            &id,
            label.as_ref(),
            true,
            *value == active,
            None::<&str>,
        )?);
    }
    let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|i| i as &dyn IsMenuItem<Wry>).collect();
    let submenu = Submenu::with_items(app, title, true, &refs)?;

    let map: HashMap<String, CheckMenuItem<Wry>> = options
        .iter()
        .zip(items.into_iter())
        .map(|((value, _), item)| ((*value).to_string(), item))
        .collect();

    Ok((submenu, map))
}

fn handle_menu_event(app: &AppHandle<Wry>, event: MenuEvent) {
    let id = event.id().as_ref().to_string();
    match id.as_str() {
        SHOW_MAIN => show_main_window(app),
        QUIT => app.exit(0),
        PICK_LANGUAGES => show_main_window(app),
        PERSISTENT_BAR => toggle_persistent_bar(app),
        RESTORE_BAR => {
            log::info!("[vibeking tray] restore floating bar — recreating chip window");
            crate::windows::recreate_chipbar(app);
        }
        other if other.starts_with(LANG_PREFIX) => {
            let value = &other[LANG_PREFIX.len()..];
            if value == "auto" || language_known(value) {
                set_language(app, value);
            }
        }
        other if other.starts_with(PROVIDER_PREFIX) => {
            let value = &other[PROVIDER_PREFIX.len()..];
            if PROVIDERS.iter().any(|(v, _)| *v == value) {
                set_provider(app, value);
            }
        }
        other if other.starts_with(REFINEMENT_PREFIX) => {
            let value = &other[REFINEMENT_PREFIX.len()..];
            set_refinement_mode(app, value);
        }
        other if other.starts_with(MIC_PREFIX) => {
            let value = &other[MIC_PREFIX.len()..];
            set_mic_device(app, value);
        }
        _ => {}
    }
}

/// Tray toggle for the always-on idle bar. Flips `persistent_bar` in the
/// settings cache, reconciles the chip-bar window to match, rebuilds the menu
/// so the check mark reflects the new state, and mirrors the value to the
/// frontend via `tray:set-persistent-bar` so the JSON store stays in sync.
fn toggle_persistent_bar(app: &AppHandle<Wry>) {
    let next = {
        let state = app.state::<SharedState>();
        let mut s = state.settings.lock();
        s.persistent_bar = !s.persistent_bar;
        s.persistent_bar
    };
    {
        let state = app.state::<SharedState>();
        let shared = state.inner().clone();
        crate::windows::reconcile_persistent_bar(app, &shared);
    }
    rebuild(app);
    let _ = app.emit("tray:set-persistent-bar", next);
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn set_language(app: &AppHandle<Wry>, lang: &str) {
    // Guard: if the current provider can't actually handle this
    // language (e.g. user picked 中文 from the menu while Parakeet is
    // active — shouldn't be visible after the per-provider filter, but
    // belt-and-braces), silently fall back to auto.
    let effective = {
        let state = app.state::<SharedState>();
        let s = state.settings.lock();
        if provider_id(s.provider) == "fluid-audio" && !parakeet_supports(lang) {
            "auto"
        } else {
            lang
        }
    };
    {
        let state = app.state::<SharedState>();
        let mut s = state.settings.lock();
        if s.stt_language != effective {
            s.stt_language = effective.to_string();
        }
    }
    sync_radio_checks(&LANG_ITEMS, effective);
    let _ = app.emit("tray:set-stt-language", effective);
}

/// Tray click handler for the Refinement-mode submenu. `value` is either
/// `REFINEMENT_OFF` (sentinel for "no refinement") or a mode id from the
/// user's settings. Unknown ids are silently ignored — the menu shape
/// already constrains the input, but be defensive against id reuse races
/// with concurrent settings edits.
fn set_refinement_mode(app: &AppHandle<Wry>, value: &str) {
    let id: Option<String> = if value == REFINEMENT_OFF {
        None
    } else {
        // Validate against the live mode list so a stale id doesn't get
        // written into settings — the chip badge would then show empty.
        let state = app.state::<SharedState>();
        let s = state.settings.lock();
        if s.refinement_modes.iter().any(|m| m.id == value) {
            Some(value.to_string())
        } else {
            return;
        }
    };

    let state = app.state::<SharedState>();
    let shared = state.inner().clone();
    crate::state::apply_active_refinement_mode(app, &shared, id);
    sync_refinement_checks(value);
    // Mirror the tray:set-* pattern so the frontend's tray.ts can persist
    // through `patchSettings` and keep the JSON store in sync.
    let _ = app.emit("tray:set-refinement-mode", value);
}

fn sync_refinement_checks(active_key: &str) {
    let Ok(guard) = REFINEMENT_ITEMS.lock() else {
        return;
    };
    let Some(map) = guard.as_ref() else {
        return;
    };
    for (key, item) in map.iter() {
        let _ = item.set_checked(key == active_key);
    }
}

/// Tray click handler for the Microphone submenu. `value` is either
/// `MIC_SYSTEM_DEFAULT` (sentinel) or a literal device name. The empty
/// string in `settings.mic_device` is the canonical "system default" — no
/// migration needed. Click handler keeps both the in-memory cache and
/// the persisted JSON in lock-step via the tray:set-* channel.
fn set_mic_device(app: &AppHandle<Wry>, value: &str) {
    let resolved = if value == MIC_SYSTEM_DEFAULT {
        String::new()
    } else {
        value.to_string()
    };
    {
        let state = app.state::<SharedState>();
        let mut s = state.settings.lock();
        if s.mic_device != resolved {
            s.mic_device = resolved.clone();
        }
    }
    // Tray menu shape can change (device just got reconnected; saved
    // device just disconnected) so rebuild rather than poking individual
    // items. Cheap — cpal enumeration is ~ms on macOS.
    rebuild(app);
    // Mirror to TS so the JSON store and Home.tsx settings page stay in
    // sync. Empty-string sentinel is preserved as-is across the wire.
    let _ = app.emit("tray:set-mic-device", resolved);
}

fn set_provider(app: &AppHandle<Wry>, provider: &str) {
    let Some(parsed) = provider_from_id(provider) else {
        return;
    };
    {
        let state = app.state::<SharedState>();
        let mut s = state.settings.lock();
        if s.provider != parsed {
            s.provider = parsed;
        }
        // Same reset the Home dropdown does — keep the two paths
        // consistent so flipping provider via tray doesn't leave a
        // silently-unsupported language hint in the next recording.
        if provider == "fluid-audio" && !parakeet_supports(&s.stt_language) {
            s.stt_language = "auto".to_string();
        }
    }
    sync_radio_checks(&PROVIDER_ITEMS, provider);
    // Provider change reshapes the language submenu (filter + selected
    // radio) — full rebuild rather than poking individual items.
    rebuild(app);
    let _ = app.emit("tray:set-provider", provider);
}

fn sync_radio_checks(slot: &Mutex<Option<HashMap<String, CheckMenuItem<Wry>>>>, active: &str) {
    let Ok(guard) = slot.lock() else { return };
    let Some(map) = guard.as_ref() else { return };
    for (value, item) in map.iter() {
        let _ = item.set_checked(value == active);
    }
}

fn provider_id(p: Provider) -> String {
    match p {
        Provider::Deepgram => "deepgram",
        Provider::Groq => "groq",
        Provider::Openai => "openai",
        Provider::Elevenlabs => "elevenlabs",
        Provider::Local => "local",
        Provider::FluidAudio => "fluid-audio",
        Provider::Qwen3 => "qwen3",
        Provider::Gemma => "gemma",
    }
    .to_string()
}

fn provider_from_id(id: &str) -> Option<Provider> {
    match id {
        "deepgram" => Some(Provider::Deepgram),
        "groq" => Some(Provider::Groq),
        "openai" => Some(Provider::Openai),
        "elevenlabs" => Some(Provider::Elevenlabs),
        "local" => Some(Provider::Local),
        "fluid-audio" => Some(Provider::FluidAudio),
        "qwen3" => Some(Provider::Qwen3),
        "gemma" => Some(Provider::Gemma),
        _ => None,
    }
}
