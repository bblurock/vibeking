//! Phase 1B ship-gate eval for the correction detector.
//!
//! Loads every fixture in `tests/fixtures/correction_detector/*.json`, runs
//! each through [`detect_correction`], tallies true/false positives and
//! negatives, then asserts:
//!
//!   - precision ≥ 80%
//!   - recall    ≥ 50%
//!
//! These thresholds are regression gates for correction learning.
//!
//! The 20 fixtures cover realistic editing scenarios across Mail / Slack /
//! Notion / Cursor / plain-editor contexts:
//!
//!   - 12 "should detect" cases (claw→Claude, casing fixes, Lev 1-4 typos,
//!     markdown-bracketed tokens, end-of-sentence, prefix-stripped variants).
//!   - 8  "should NOT detect" cases (multi-word changes, punctuation-only
//!     edits, whole-message rewrites, Lev≥5, add/remove token, sub-3-char
//!     `from`, pure numeric).
//!
//! The "should NOT detect" half exists to stress-test precision — a detector
//! that fires indiscriminately would fail precision even with perfect recall.
//!
//! Run with:
//!     cargo test --test correction_detector_eval -- --nocapture
//!
//! `--nocapture` makes the per-fixture tally and gate verdict visible in CI.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use vibeking_lib::corrections::detect_correction;

#[derive(Debug, Deserialize)]
struct Fixture {
    name: String,
    #[allow(dead_code)]
    context: String,
    /// Either "detected" or "not_detected".
    expected: String,
    pasted: String,
    current_field: String,
    original_field: Option<String>,
    expected_from: Option<String>,
    expected_to: Option<String>,
}

#[derive(Debug, Default)]
struct Tally {
    true_positive: usize,
    false_positive: usize,
    true_negative: usize,
    false_negative: usize,
}

const SHIP_GATE_PRECISION: f64 = 0.80;
const SHIP_GATE_RECALL: f64 = 0.50;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("correction_detector")
}

fn load_fixtures() -> Vec<(PathBuf, Fixture)> {
    let dir = fixtures_dir();
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    entries.sort();
    entries
        .into_iter()
        .map(|p| {
            let raw = fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("could not read {}: {e}", p.display()));
            let fx: Fixture = serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("could not parse {}: {e}", p.display()));
            (p, fx)
        })
        .collect()
}

#[test]
fn fixtures_directory_has_expected_files() {
    let fixtures = load_fixtures();
    // 20 single-token fixtures from Phase 1B core (T17) + bigram fixtures
    // added in Phase 1B+ for the multi-token detector expansion.
    assert!(
        fixtures.len() >= 20,
        "expected at least 20 fixtures in {}, got {}",
        fixtures_dir().display(),
        fixtures.len()
    );
}

#[test]
fn correction_detector_eval_meets_ship_gate() {
    let fixtures = load_fixtures();
    let mut tally = Tally::default();
    let mut failures: Vec<String> = Vec::new();

    for (_path, fx) in &fixtures {
        let got = detect_correction(&fx.pasted, &fx.current_field, fx.original_field.as_deref());

        match (fx.expected.as_str(), got) {
            ("detected", Some(c)) => {
                let from_match = fx.expected_from.as_ref().map_or(true, |f| f == &c.from);
                let to_match = fx.expected_to.as_ref().map_or(true, |t| t == &c.to);
                if from_match && to_match {
                    tally.true_positive += 1;
                } else {
                    // Detected something, but the wrong swap. Counts as a
                    // false positive (we fired on the wrong token) AND a
                    // false negative (we missed the expected one) — we
                    // record as false positive only to keep the math
                    // conservative on the precision side.
                    tally.false_positive += 1;
                    failures.push(format!(
                        "{}: detected wrong swap. expected from={:?} to={:?}, got from={:?} to={:?}",
                        fx.name, fx.expected_from, fx.expected_to, c.from, c.to
                    ));
                }
            }
            ("detected", None) => {
                tally.false_negative += 1;
                failures.push(format!("{}: expected detection, got None", fx.name));
            }
            ("not_detected", None) => {
                tally.true_negative += 1;
            }
            ("not_detected", Some(c)) => {
                tally.false_positive += 1;
                failures.push(format!(
                    "{}: expected no detection, got {:?} -> {:?}",
                    fx.name, c.from, c.to
                ));
            }
            (other, _) => panic!(
                "invalid `expected` value in fixture {}: {:?} (must be \"detected\" or \"not_detected\")",
                fx.name, other
            ),
        }
    }

    let tp = tally.true_positive as f64;
    let fp = tally.false_positive as f64;
    let fn_ = tally.false_negative as f64;

    // Standard guards: if denominator is zero we vacuously pass that side.
    let precision = if tp + fp == 0.0 { 1.0 } else { tp / (tp + fp) };
    let recall = if tp + fn_ == 0.0 {
        1.0
    } else {
        tp / (tp + fn_)
    };

    println!();
    println!("=== Correction-Detector Eval (Phase 1B ship gate) ===");
    println!("fixtures:  {}", fixtures.len());
    println!(
        "tally:     TP={} FP={} TN={} FN={}",
        tally.true_positive, tally.false_positive, tally.true_negative, tally.false_negative
    );
    println!(
        "precision: {:.1}%  (gate: >={:.0}%)",
        precision * 100.0,
        SHIP_GATE_PRECISION * 100.0
    );
    println!(
        "recall:    {:.1}%  (gate: >={:.0}%)",
        recall * 100.0,
        SHIP_GATE_RECALL * 100.0
    );
    if !failures.is_empty() {
        println!();
        println!("failures:");
        for f in &failures {
            println!("  - {}", f);
        }
    }
    let gate_passes = precision >= SHIP_GATE_PRECISION && recall >= SHIP_GATE_RECALL;
    println!();
    println!("verdict:   {}", if gate_passes { "PASS" } else { "FAIL" });
    println!();

    assert!(
        precision >= SHIP_GATE_PRECISION,
        "precision {:.1}% below {:.0}% ship gate",
        precision * 100.0,
        SHIP_GATE_PRECISION * 100.0
    );
    assert!(
        recall >= SHIP_GATE_RECALL,
        "recall {:.1}% below {:.0}% ship gate",
        recall * 100.0,
        SHIP_GATE_RECALL * 100.0
    );
}
