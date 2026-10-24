//! Tiny translation layer: every user-facing string goes through [`tr`] / [`trf`].
//!
//! Locales are JSON maps embedded at compile time. The active locale is picked
//! from the environment once at startup; missing keys fall back to English,
//! then to the key itself so a gap is visible instead of silent.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Embedded locale files: (language code, JSON source). English comes first
/// and is the fallback for missing keys.
const LOCALES: &[(&str, &str)] = &[
    ("en", include_str!("../locales/en.json")),
    ("es", include_str!("../locales/es.json")),
    ("fr", include_str!("../locales/fr.json")),
    ("de", include_str!("../locales/de.json")),
];
const FALLBACK: usize = 0;

static CATALOGS: OnceLock<Vec<HashMap<String, String>>> = OnceLock::new();
static ACTIVE: AtomicUsize = AtomicUsize::new(FALLBACK);

fn parse(src: &str) -> HashMap<String, String> {
    // The files are compiled in, so a parse failure is a build-time bug caught by tests.
    serde_json::from_str(src).unwrap_or_default()
}

fn catalogs() -> &'static [HashMap<String, String>] {
    CATALOGS.get_or_init(|| LOCALES.iter().map(|(_, src)| parse(src)).collect())
}

/// Extracts the language code from a POSIX locale such as `de_DE.UTF-8`.
fn language_code(locale: &str) -> &str {
    locale.split(['_', '.', '@', '-']).next().unwrap_or("en")
}

fn system_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|v| !v.is_empty())
        .map(|v| language_code(&v).to_owned())
        .unwrap_or_else(|| "en".to_owned())
}

/// Available languages as (code, native name).
pub fn languages() -> Vec<(&'static str, &'static str)> {
    LOCALES
        .iter()
        .zip(catalogs())
        .map(|((code, _), map)| {
            (
                *code,
                map.get("language.name").map_or(*code, String::as_str),
            )
        })
        .collect()
}

/// Switches the UI language; `None` follows the system locale. Unknown
/// codes fall back to English.
pub fn set_language(code: Option<&str>) {
    let wanted = code.map_or_else(system_language, str::to_owned);
    let index = LOCALES
        .iter()
        .position(|(c, _)| *c == wanted)
        .unwrap_or(FALLBACK);
    ACTIVE.store(index, Ordering::Relaxed);
}

/// Translates `key`.
pub fn tr(key: &'static str) -> &'static str {
    let all = catalogs();
    all[ACTIVE.load(Ordering::Relaxed)]
        .get(key)
        .or_else(|| all[FALLBACK].get(key))
        .map_or(key, String::as_str)
}

/// Translates `key` and substitutes `{name}` placeholders.
pub fn trf(key: &'static str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    args.iter().fold(tr(key).to_owned(), |text, (name, value)| {
        text.replace(&format!("{{{name}}}"), &value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_locale_parses() {
        for (code, src) in LOCALES {
            let parsed: Result<HashMap<String, String>, _> = serde_json::from_str(src);
            assert!(parsed.is_ok(), "locale {code} is not valid JSON");
        }
    }

    /// Translations may lag behind, but must not invent keys or drop placeholders.
    #[test]
    fn translations_match_english_keys_and_placeholders() {
        let en = &catalogs()[FALLBACK];
        let placeholders = |text: &str| {
            let mut found: Vec<String> = text
                .split('{')
                .skip(1)
                .filter_map(|p| p.split_once('}'))
                .map(|(n, _)| n.to_owned())
                .collect();
            found.sort();
            found
        };
        for ((code, _), map) in LOCALES.iter().zip(catalogs()).skip(1) {
            for (key, text) in map {
                let english = en
                    .get(key)
                    .unwrap_or_else(|| panic!("{code}: unknown key {key}"));
                assert_eq!(placeholders(text), placeholders(english), "{code}: {key}");
            }
        }
    }

    #[test]
    fn language_code_handles_posix_forms() {
        assert_eq!(language_code("de_DE.UTF-8"), "de");
        assert_eq!(language_code("en"), "en");
        assert_eq!(language_code("sr@latin"), "sr");
    }

    #[test]
    fn placeholders_are_substituted() {
        let out = trf("queue.summary", &[("done", &2), ("total", &5)]);
        assert!(out.contains('2') && out.contains('5'), "{out}");
    }

    /// Every `tr("…")`/`trf("…")` key used in the sources must exist in English.
    #[test]
    fn all_used_keys_exist() {
        let en = &catalogs()[FALLBACK];
        let mut missing = HashSet::new();
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        for entry in std::fs::read_dir(dir).expect("src dir") {
            let path = entry.expect("dir entry").path();
            if path.ends_with("i18n.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("source file");
            for marker in ["tr(\"", "trf(\""] {
                for (at, _) in text.match_indices(marker) {
                    // Skip longer identifiers that merely end in `tr`, e.g. `push_str(`.
                    let prev = text[..at].chars().next_back().unwrap_or(' ');
                    if prev.is_alphanumeric() || prev == '_' {
                        continue;
                    }
                    let rest = &text[at + marker.len()..];
                    let key = rest.split('"').next().unwrap_or_default();
                    if !key.is_empty() && !en.contains_key(key) {
                        missing.insert(key.to_owned());
                    }
                }
            }
        }
        assert!(missing.is_empty(), "missing locale keys: {missing:?}");
    }
}
