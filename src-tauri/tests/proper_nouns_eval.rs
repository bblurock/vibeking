//! Integration eval harness for the proper-noun extractor.
//!
//! Loads every fixture in `tests/fixtures/proper_nouns/*.json`, runs the
//! extractor over the `screen_text`, and asserts:
//!   - every token in `expected_include` is present (case-sensitive)
//!   - no token in `expected_exclude` is present
//!
//! Failures print the fixture name, missing tokens, unexpected tokens, and the
//! full extractor output so we can iterate on the rules.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use vibeking_lib::proper_nouns::extract_candidates;

#[derive(Debug, Deserialize)]
struct Fixture {
    name: String,
    #[allow(dead_code)]
    source_description: String,
    screen_text: String,
    expected_include: Vec<String>,
    expected_exclude: Vec<String>,
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("proper_nouns")
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
            let f: Fixture = serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("could not parse {}: {e}", p.display()));
            (p, f)
        })
        .collect()
}

#[test]
fn fixtures_directory_has_eight_files() {
    let fixtures = load_fixtures();
    assert_eq!(
        fixtures.len(),
        8,
        "expected exactly 8 fixtures in {}, got {}",
        fixtures_dir().display(),
        fixtures.len()
    );
}

#[test]
fn every_fixture_passes_include_and_exclude_assertions() {
    let fixtures = load_fixtures();
    let mut failures: Vec<String> = Vec::new();

    for (path, fx) in &fixtures {
        let out = extract_candidates(&fx.screen_text, 50);

        let missing: Vec<&String> = fx
            .expected_include
            .iter()
            .filter(|tok| !out.iter().any(|o| o == *tok))
            .collect();
        let unexpected: Vec<&String> = fx
            .expected_exclude
            .iter()
            .filter(|tok| out.iter().any(|o| o == *tok))
            .collect();

        if !missing.is_empty() || !unexpected.is_empty() {
            failures.push(format!(
                "\n--- fixture failed: {} ({})\n  missing (expected_include not in output): {:?}\n  unexpected (expected_exclude leaked into output): {:?}\n  full extractor output: {:?}",
                fx.name,
                path.display(),
                missing,
                unexpected,
                out,
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} fixture(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
