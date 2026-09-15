// Per-engine supported STT language sets.
//
// The single global STT_LANGUAGES list was misleading: Parakeet TDT v3 is
// a 25-language *European* model with no CJK support, while whisper.cpp
// and the cloud providers handle the Whisper-99 set. Letting a user pick
// "中文 / Chinese" while Parakeet is the active engine produced silent
// failures (Parakeet ignores the language hint entirely and auto-detects
// across its own 25 — Chinese audio comes back as gibberish).
//
// supportedSttLanguages(provider) returns the union of:
//   - the "auto" sentinel at the top
//   - the codes the engine actually handles
// joined with the master label table below.

import type { Provider } from "./settings";

export type SttLanguageOption = { value: string; label: string };

/// Master label table. Codes are ISO 639-1 (with the few three-letter
/// extensions Whisper itself uses: haw, yue). Labels follow the existing
/// `"Native · English"` pattern.
const LABELS: Record<string, string> = {
  af: "Afrikaans",
  am: "አማርኛ · Amharic",
  ar: "العربية · Arabic",
  as: "অসমীয়া · Assamese",
  az: "Azərbaycanca · Azerbaijani",
  ba: "Башҡортса · Bashkir",
  be: "Беларуская · Belarusian",
  bg: "Български · Bulgarian",
  bn: "বাংলা · Bengali",
  bo: "བོད་སྐད་ · Tibetan",
  br: "Brezhoneg · Breton",
  bs: "Bosanski · Bosnian",
  ca: "Català · Catalan",
  cs: "Čeština · Czech",
  cy: "Cymraeg · Welsh",
  da: "Dansk · Danish",
  de: "Deutsch · German",
  el: "Ελληνικά · Greek",
  en: "English",
  es: "Español · Spanish",
  et: "Eesti · Estonian",
  eu: "Euskara · Basque",
  fa: "فارسی · Persian",
  fi: "Suomi · Finnish",
  fo: "Føroyskt · Faroese",
  fr: "Français · French",
  gl: "Galego · Galician",
  gu: "ગુજરાતી · Gujarati",
  ha: "Hausa",
  haw: "ʻŌlelo Hawaiʻi · Hawaiian",
  he: "עברית · Hebrew",
  hi: "हिन्दी · Hindi",
  hr: "Hrvatski · Croatian",
  ht: "Kreyòl Ayisyen · Haitian Creole",
  hu: "Magyar · Hungarian",
  hy: "Հայերեն · Armenian",
  id: "Bahasa Indonesia",
  is: "Íslenska · Icelandic",
  it: "Italiano · Italian",
  ja: "日本語 · Japanese",
  jw: "Basa Jawa · Javanese",
  ka: "ქართული · Georgian",
  kk: "Қазақша · Kazakh",
  km: "ខ្មែរ · Khmer",
  kn: "ಕನ್ನಡ · Kannada",
  ko: "한국어 · Korean",
  la: "Latina · Latin",
  lb: "Lëtzebuergesch · Luxembourgish",
  ln: "Lingála · Lingala",
  lo: "ລາວ · Lao",
  lt: "Lietuvių · Lithuanian",
  lv: "Latviešu · Latvian",
  mg: "Malagasy",
  mi: "Te Reo Māori · Maori",
  mk: "Македонски · Macedonian",
  ml: "മലയാളം · Malayalam",
  mn: "Монгол · Mongolian",
  mr: "मराठी · Marathi",
  ms: "Bahasa Melayu · Malay",
  mt: "Malti · Maltese",
  my: "မြန်မာ · Burmese",
  ne: "नेपाली · Nepali",
  nl: "Nederlands · Dutch",
  nn: "Nynorsk · Norwegian Nynorsk",
  no: "Norsk · Norwegian",
  oc: "Occitan",
  pa: "ਪੰਜਾਬੀ · Punjabi",
  pl: "Polski · Polish",
  ps: "پښتو · Pashto",
  pt: "Português · Portuguese",
  ro: "Română · Romanian",
  ru: "Русский · Russian",
  sa: "संस्कृतम् · Sanskrit",
  sd: "سنڌي · Sindhi",
  si: "සිංහල · Sinhala",
  sk: "Slovenčina · Slovak",
  sl: "Slovenščina · Slovenian",
  sn: "ChiShona · Shona",
  so: "Soomaali · Somali",
  sq: "Shqip · Albanian",
  sr: "Српски · Serbian",
  su: "Basa Sunda · Sundanese",
  sv: "Svenska · Swedish",
  sw: "Kiswahili · Swahili",
  ta: "தமிழ் · Tamil",
  te: "తెలుగు · Telugu",
  tg: "Тоҷикӣ · Tajik",
  th: "ไทย · Thai",
  tk: "Türkmençe · Turkmen",
  tl: "Tagalog",
  tr: "Türkçe · Turkish",
  tt: "Татарча · Tatar",
  uk: "Українська · Ukrainian",
  ur: "اردو · Urdu",
  uz: "Oʻzbekcha · Uzbek",
  vi: "Tiếng Việt · Vietnamese",
  yi: "ייִדיש · Yiddish",
  yo: "Yorùbá · Yoruba",
  yue: "粵語 · Cantonese",
  zh: "中文 · Chinese",
};

/// Whisper-99 set. Source: openai/whisper tokenizer language table — the
/// full set that both whisper.cpp and the Whisper-derived cloud APIs
/// (Groq, OpenAI, Deepgram Nova-3 multilingual, ElevenLabs Scribe) accept.
const WHISPER_99: readonly string[] = [
  "af", "am", "ar", "as", "az", "ba", "be", "bg", "bn", "bo", "br", "bs",
  "ca", "cs", "cy", "da", "de", "el", "en", "es", "et", "eu", "fa", "fi",
  "fo", "fr", "gl", "gu", "ha", "haw", "he", "hi", "hr", "ht", "hu", "hy",
  "id", "is", "it", "ja", "jw", "ka", "kk", "km", "kn", "ko", "la", "lb",
  "ln", "lo", "lt", "lv", "mg", "mi", "mk", "ml", "mn", "mr", "ms", "mt",
  "my", "ne", "nl", "nn", "no", "oc", "pa", "pl", "ps", "pt", "ro", "ru",
  "sa", "sd", "si", "sk", "sl", "sn", "so", "sq", "sr", "su", "sv", "sw",
  "ta", "te", "tg", "th", "tk", "tl", "tr", "tt", "uk", "ur", "uz", "vi",
  "yi", "yo", "yue", "zh",
];

/// Parakeet TDT v3 — the 25 European Union–region languages the model
/// auto-detects across. Notably *no* CJK (zh, ja, ko) and no Arabic /
/// Hebrew / Hindi. The `lang` arg is ignored by the engine; this list is
/// purely for UI honesty about what the model can actually transcribe.
const PARAKEET_25: readonly string[] = [
  "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu",
  "it", "lv", "lt", "mt", "pl", "pt", "ro", "ru", "sk", "sl", "es", "sv",
  "uk",
];

/// Qwen3-ASR languages — must stay in sync with `tray.rs::QWEN3_LANGS`.
/// Multilingual with first-class CJK (zh, yue, ja, ko) plus ~26 others.
const QWEN3_LANGS: readonly string[] = [
  "zh", "yue", "en", "ja", "ko", "vi", "th", "id", "ms", "ar", "hi", "fa",
  "tr", "ru", "de", "fr", "es", "pt", "it", "nl", "sv", "da", "fi", "pl",
  "cs", "el", "hu", "ro", "mk", "tl",
];

function toOptions(codes: readonly string[]): SttLanguageOption[] {
  return codes
    .map((code) => ({
      value: code,
      label: LABELS[code] ?? code,
    }))
    .sort((a, b) => a.label.localeCompare(b.label));
}

/// Gemma 4 audio languages — must stay in sync with `tray.rs::GEMMA_LANGS`.
/// Gemma's multimodal coverage (~35) is narrower than its 140-language text set.
const GEMMA_LANGS: readonly string[] = [
  "en", "zh", "ja", "ko", "de", "fr", "es", "pt", "it", "nl", "ru", "ar",
  "hi", "id", "vi", "th", "tr", "pl", "uk", "cs", "el", "hu", "ro", "sv",
  "da", "fi", "he", "fa", "ta", "te", "bn", "ml", "sw",
];

const WHISPER_OPTIONS = toOptions(WHISPER_99);
const PARAKEET_OPTIONS = toOptions(PARAKEET_25);
const QWEN3_OPTIONS = toOptions(QWEN3_LANGS);
const GEMMA_OPTIONS = toOptions(GEMMA_LANGS);

function withAuto(
  options: SttLanguageOption[],
  autoLabel: string,
): SttLanguageOption[] {
  return [{ value: "auto", label: autoLabel }, ...options];
}

/// Returns the language options the given provider supports, prefixed
/// with the "auto" sentinel. Cloud providers and local whisper.cpp share
/// the Whisper-99 set; fluid-audio is constrained to Parakeet's 25.
export function supportedSttLanguages(
  provider: Provider,
  autoLabel = "Auto-detect",
): SttLanguageOption[] {
  if (provider === "fluid-audio") {
    return withAuto(PARAKEET_OPTIONS, autoLabel);
  }
  if (provider === "qwen3") {
    return withAuto(QWEN3_OPTIONS, autoLabel);
  }
  if (provider === "gemma") {
    return withAuto(GEMMA_OPTIONS, autoLabel);
  }
  return withAuto(WHISPER_OPTIONS, autoLabel);
}

/// The plain English name for a language code, derived from the master
/// LABELS table: take the part after the "·" separator (the English half of
/// the "Native · English" pattern), trimmed. Codes whose label is English-
/// only (no separator, e.g. "Afrikaans") return that whole label. Falls back
/// to the uppercased code if the label is missing entirely.
export function englishLanguageName(code: string): string {
  const label = LABELS[code];
  if (!label) return code.toUpperCase();
  const sep = label.indexOf("·");
  return sep >= 0 ? label.slice(sep + 1).trim() : label.trim();
}

/// The supported languages for an engine, rendered as readable English
/// names sorted alphabetically. Driven by the same const arrays the picker
/// dropdown uses, so the displayed coverage stays truthful to what each
/// engine actually transcribes.
export function englishLanguageNames(
  engine: "whisper" | "parakeet" | "qwen3" | "gemma",
): string[] {
  const set =
    engine === "parakeet"
      ? PARAKEET_25
      : engine === "qwen3"
        ? QWEN3_LANGS
        : engine === "gemma"
          ? GEMMA_LANGS
          : WHISPER_99;
  return set
    .map(englishLanguageName)
    .sort((a, b) => a.localeCompare(b));
}

/// True iff `code` is in the supported set for `provider`. Used to decide
/// whether a stored sttLanguage survives a provider switch (otherwise we
/// reset it to "auto" silently).
export function sttLanguageSupported(provider: Provider, code: string): boolean {
  if (code === "auto") return true;
  const set =
    provider === "fluid-audio"
      ? PARAKEET_25
      : provider === "qwen3"
        ? QWEN3_LANGS
        : provider === "gemma"
          ? GEMMA_LANGS
          : WHISPER_99;
  return set.includes(code);
}
