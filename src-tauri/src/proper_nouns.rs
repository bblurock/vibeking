//! Proper-noun candidate extractor for screen-context biasing.
//!
//! Pure logic — no I/O, no FFI. Given arbitrary screen text, returns a deduped,
//! ordered list of likely proper nouns (people, brands, libraries, identifiers
//! with digits, mixed-case product names, etc.) suitable for feeding Whisper as
//! `initial_prompt`.

/// Punctuation characters we split on (in addition to whitespace).
///
/// Notably absent: `-` and `_`. This keeps tokens like "GPT-5", "T-Mobile",
/// "Foo_Bar", and "snake_case" whole so the digit / mixed-case rules can fire.
const SPLIT_PUNCT: &[char] = &[
    '.', ',', '!', '?', ';', ':', '(', ')', '[', ']', '{', '}', '<', '>', '"', '\'', '`', '/',
    '\\', '|', '&', '*', '#', '@',
];

/// Punctuation that should be stripped from the leading/trailing edges of a
/// token if it somehow survives splitting (defensive — most of these are
/// already split delimiters, but keep this in sync if SPLIT_PUNCT changes).
const STRIP_EDGE: &[char] = &[
    '.', ',', '!', '?', ';', ':', '(', ')', '[', ']', '{', '}', '<', '>', '"', '\'', '`', '/',
    '\\', '|', '&', '*', '#', '@', '-', '_',
];

/// Sentence-terminating punctuation. A run of these followed by whitespace
/// indicates the next non-space token is sentence-initial.
const SENTENCE_END: &[char] = &['.', '!', '?'];

/// Extract proper-noun candidates from arbitrary screen text.
///
/// See module docs and the unit tests for the full ruleset. Output is
/// deduplicated case-insensitively, ordered by first appearance, capped at
/// `max` entries.
pub fn extract_candidates(text: &str, max: usize) -> Vec<String> {
    if max == 0 || text.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<String> = Vec::new();
    let mut seen_lower: std::collections::HashSet<String> = std::collections::HashSet::new();

    // We need both the token text and whether the preceding non-whitespace
    // character was a sentence terminator. Walk byte-by-byte (chars actually)
    // to preserve that context.
    let mut chars = text.chars().peekable();
    let mut token = String::new();
    // Treat the start of the document as sentence-initial: if the very first
    // token is capitalized, we can't tell common-word-vs-proper-noun, so it
    // gets filtered out the same way a sentence-initial token would.
    let mut next_is_sentence_initial = true;
    let mut last_non_ws_was_sentence_end = false;

    while let Some(c) = chars.next() {
        if c.is_whitespace() || SPLIT_PUNCT.contains(&c) {
            // Flush the in-progress token (if any).
            if !token.is_empty() {
                let is_sentence_initial = next_is_sentence_initial;
                // After consuming a token, the next token's sentence-initial
                // status depends on what we saw between this token and the
                // next one. Default to false unless a sentence terminator is
                // seen below.
                next_is_sentence_initial = false;
                consider(&token, is_sentence_initial, &mut out, &mut seen_lower, max);
                token.clear();
                if out.len() >= max {
                    return out;
                }
            }
            // Update sentence-state machine. We only treat `. ! ?` followed by
            // whitespace as a real sentence end (matches the spec). Track the
            // most recent non-whitespace split char so we know whether the
            // current whitespace flips us into sentence-initial mode.
            if SENTENCE_END.contains(&c) {
                last_non_ws_was_sentence_end = true;
            } else if c.is_whitespace() {
                if last_non_ws_was_sentence_end {
                    next_is_sentence_initial = true;
                    last_non_ws_was_sentence_end = false;
                }
            } else {
                // Some other split punct that isn't a sentence terminator.
                last_non_ws_was_sentence_end = false;
            }
        } else {
            token.push(c);
            last_non_ws_was_sentence_end = false;
        }
    }
    if !token.is_empty() {
        consider(
            &token,
            next_is_sentence_initial,
            &mut out,
            &mut seen_lower,
            max,
        );
    }

    out
}

fn consider(
    raw: &str,
    is_sentence_initial: bool,
    out: &mut Vec<String>,
    seen_lower: &mut std::collections::HashSet<String>,
    max: usize,
) {
    if out.len() >= max {
        return;
    }
    let trimmed = raw.trim_matches(|c: char| STRIP_EDGE.contains(&c));
    if trimmed.len() < 2 {
        return;
    }

    // Reject obvious identifier / query-string shapes like `discount_any=1`,
    // `?ref=home`, `id=42`. Real proper nouns never contain `=`.
    if trimmed.contains('=') {
        return;
    }

    let has_digit = trimmed.chars().any(|c| c.is_ascii_digit());
    let has_lower = trimmed.chars().any(|c| c.is_lowercase());
    let has_upper = trimmed.chars().any(|c| c.is_uppercase());
    let first_char = trimmed.chars().next().unwrap();
    let first_is_upper = first_char.is_uppercase();

    // Pure numeric → reject.
    if !has_lower && !has_upper {
        // No letters at all. Could be digits only, or punctuation/symbols.
        return;
    }

    // Rule (c): contains a digit AND len >= 2. This is the override-all
    // path — keep it even if it'd otherwise fail the all-caps-short filter.
    let matches_c = has_digit && trimmed.chars().count() >= 2;

    // Reject all-uppercase tokens with no lowercase letters (regardless of
    // length), UNLESS rule (c) is also true. This kills "URL", "API", "CSS"
    // but also catches "MY_CONST" (no lowercase, length 8) which we want to
    // reject per the spec.
    if !has_lower && !matches_c {
        return;
    }

    // Rule (a): capitalized AND not sentence-initial.
    // "Capitalized" = first char uppercase, contains at least one lowercase.
    let matches_a = first_is_upper && has_lower && !is_sentence_initial;

    // Rule (b): mixed case where first char is NOT uppercase-followed-by-
    // -all-lowercase-style "Capitalized" — i.e. the "iPhone" / "vLLM" shape.
    // Simplest precise definition: has both upper and lower, AND first char
    // is NOT uppercase (so "Apple" falls out of (b), into (a); "iPhone",
    // "vLLM", "macOS" land here).
    let matches_b = has_upper && has_lower && !first_is_upper;

    let keep = matches_a || matches_b || matches_c;
    if !keep {
        return;
    }

    // Belt-and-suspenders short-all-caps filter (already covered by the
    // has_lower check above, but kept for explicit spec coverage):
    // all uppercase letters AND length <= 3 → reject unless rule (c).
    let all_upper_letters = trimmed
        .chars()
        .filter(|c| c.is_alphabetic())
        .all(|c| c.is_uppercase());
    if all_upper_letters && !has_lower && trimmed.chars().count() <= 3 && !matches_c {
        return;
    }

    // Dedupe case-insensitively, keep first-seen casing.
    let key = trimmed.to_lowercase();
    if seen_lower.insert(key) {
        out.push(trimmed.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_returns_empty() {
        assert!(extract_candidates("", 50).is_empty());
    }

    #[test]
    fn rejects_identifier_with_equals_sign() {
        let out = extract_candidates(
            "Visit https://example.com?discount_any=1 to learn more about Kubernetes",
            50,
        );
        assert!(
            !out.iter().any(|s| s.contains('=')),
            "tokens with `=` should be rejected: got {out:?}"
        );
        assert!(out.iter().any(|s| s == "Kubernetes"));
    }

    #[test]
    fn rejects_sentence_initial_but_keeps_later_proper_nouns() {
        // "Katherine" is first token => sentence-initial => filtered.
        // "Karpathy" and "Kubernetes" are mid-sentence => kept.
        let out = extract_candidates("Katherine sent Karpathy a Kubernetes deployment.", 50);
        assert!(out.contains(&"Karpathy".to_string()), "got {:?}", out);
        assert!(out.contains(&"Kubernetes".to_string()), "got {:?}", out);
        assert!(!out.contains(&"Katherine".to_string()), "got {:?}", out);
    }

    #[test]
    fn mixed_case_tokens_kept() {
        let out = extract_candidates("I love iPhone and MacBook", 50);
        assert!(out.contains(&"iPhone".to_string()), "got {:?}", out);
        assert!(out.contains(&"MacBook".to_string()), "got {:?}", out);
        // "I" is length 1 → rejected. "love" is lowercase first letter and
        // has no upper → fails all rules.
        assert!(!out.contains(&"I".to_string()));
        assert!(!out.contains(&"love".to_string()));
    }

    #[test]
    fn digit_tokens_kept() {
        let out = extract_candidates("Try GPT-5 and Llama3.2 with vLLM", 50);
        // GPT-5: hyphen not in split set → kept whole. Has digit + len>=2 → rule (c).
        assert!(out.contains(&"GPT-5".to_string()), "got {:?}", out);
        // Llama3.2 → splits at '.' → "Llama3" and "2".
        assert!(out.contains(&"Llama3".to_string()), "got {:?}", out);
        // vLLM: mixed case, first char lowercase → rule (b).
        assert!(out.contains(&"vLLM".to_string()), "got {:?}", out);
        // "Try" is sentence-initial → filtered.
        assert!(!out.contains(&"Try".to_string()));
    }

    #[test]
    fn rejects_short_all_caps() {
        let out = extract_candidates("Use the API, URL, and CSS classes", 50);
        assert!(!out.contains(&"API".to_string()), "got {:?}", out);
        assert!(!out.contains(&"URL".to_string()), "got {:?}", out);
        assert!(!out.contains(&"CSS".to_string()), "got {:?}", out);
        // "classes" is lowercase only → fails all rules.
        assert!(!out.contains(&"classes".to_string()));
    }

    #[test]
    fn dedupe_case_insensitive_preserves_first_seen() {
        let out = extract_candidates(
            "I met Katherine. She met katherine again. Then KATHERINE called.",
            50,
        );
        let kats: Vec<&String> = out
            .iter()
            .filter(|s| s.to_lowercase() == "katherine")
            .collect();
        assert_eq!(kats.len(), 1, "got {:?}", out);
        assert_eq!(kats[0], "Katherine", "got {:?}", out);
    }

    #[test]
    fn cap_enforcement() {
        // 100 distinct, all mid-sentence so rule (a) fires.
        let mut text = String::from("Lead. ");
        for i in 0..100 {
            text.push_str(&format!("met Name{} and ", i));
        }
        let out = extract_candidates(&text, 5);
        assert_eq!(out.len(), 5, "got {:?}", out);
    }

    #[test]
    fn rejects_pure_numeric() {
        let out = extract_candidates("Year 2026 was good", 50);
        assert!(!out.contains(&"2026".to_string()), "got {:?}", out);
    }

    #[test]
    fn strips_punctuation_residue() {
        // Quotes + comma should be split off, leaving bare tokens.
        let out = extract_candidates(
            r#"Lead. We used "Kubernetes," and "OpenZeppelin" today."#,
            50,
        );
        assert!(out.contains(&"Kubernetes".to_string()), "got {:?}", out);
        assert!(out.contains(&"OpenZeppelin".to_string()), "got {:?}", out);
    }

    #[test]
    fn underscore_handling() {
        // "MY_CONST": no lowercase letters → rejected.
        // "Foo_Bar": has lowercase + first char upper, mid-sentence → kept via rule (a).
        let out = extract_candidates("Lead. The MY_CONST and Foo_Bar are constants", 50);
        assert!(!out.contains(&"MY_CONST".to_string()), "got {:?}", out);
        assert!(out.contains(&"Foo_Bar".to_string()), "got {:?}", out);
    }

    #[test]
    fn url_with_digit_kept() {
        // Rule (c) override: "URL2" has digit + len>=2 → kept even though
        // it'd be filtered as short-all-caps without the digit.
        let out = extract_candidates("Lead. Hit URL2 for details", 50);
        assert!(out.contains(&"URL2".to_string()), "got {:?}", out);
    }

    #[test]
    fn sentence_boundary_after_exclamation_and_question() {
        // "Karpathy" appears mid-sentence in "We saw Karpathy." → kept.
        // After "!" the next token "Yang" is sentence-initial → filtered.
        // After "?" the next "Yang" is sentence-initial → filtered.
        // So "Yang" never lands in output here — confirming "!" and "?" both
        // function as sentence terminators.
        let out = extract_candidates(
            "Lead. We saw Karpathy. Karpathy waved! Yang did too? Yang waved back.",
            50,
        );
        assert!(out.contains(&"Karpathy".to_string()), "got {:?}", out);
        assert!(!out.contains(&"Yang".to_string()), "got {:?}", out);
    }

    #[test]
    fn first_token_filtered_even_if_capitalized() {
        // Document-start token treated as sentence-initial.
        let out = extract_candidates("Apple shipped a new model", 50);
        assert!(!out.contains(&"Apple".to_string()), "got {:?}", out);
    }

    #[test]
    fn stable_order_first_appearance() {
        let out = extract_candidates(
            "Lead. met Bravo then Alpha then Bravo again then Charlie.",
            50,
        );
        let bravo = out
            .iter()
            .position(|s| s == "Bravo")
            .expect("bravo missing");
        let alpha = out
            .iter()
            .position(|s| s == "Alpha")
            .expect("alpha missing");
        let charlie = out
            .iter()
            .position(|s| s == "Charlie")
            .expect("charlie missing");
        assert!(bravo < alpha, "out={:?}", out);
        assert!(alpha < charlie, "out={:?}", out);
    }
}
