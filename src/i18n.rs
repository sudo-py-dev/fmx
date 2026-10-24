//! Tiny translation layer: every user-facing string goes through [`tr`] / [`trf`].
//!
//! Locales are JSON maps embedded at compile time. The active locale is picked
//! from the environment once at startup; missing keys fall back to English,
//! then to the key itself so a gap is visible instead of silent.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Embedded locale files: (language code, JSON source).
const LOCALES: &[(&str, &str)] = &[("en", include_str!("../locales/en.json"))];
const FALLBACK: &str = "en";

struct Catalog {
    active: HashMap<String, String>,
    fallback: HashMap<String, String>,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();

fn parse(src: &str) -> HashMap<String, String> {
    // The files are compiled in, so a parse failure is a build-time bug caught by tests.
    serde_json::from_str(src).unwrap_or_default()
}

fn source_for(lang: &str) -> Option<&'static str> {
    LOCALES
        .iter()
        .find(|(code, _)| *code == lang)
        .map(|(_, src)| *src)
}

/// Extracts the language code from a POSIX locale such as `de_DE.UTF-8`.
fn language_code(locale: &str) -> &str {
    locale
        .split(['_', '.', '@', '-'])
        .next()
        .unwrap_or(FALLBACK)
}

fn system_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|v| !v.is_empty())
        .map(|v| language_code(&v).to_owned())
        .unwrap_or_else(|| FALLBACK.to_owned())
}

/// Selects the locale. Call once at startup; later calls are ignored.
pub fn init(lang_override: Option<&str>) {
    let lang = lang_override.map_or_else(system_language, str::to_owned);
    let fallback_src = source_for(FALLBACK).unwrap_or("{}");
    let active_src = source_for(&lang).unwrap_or(fallback_src);
    let _ = CATALOG.set(Catalog {
        active: parse(active_src),
        fallback: parse(fallback_src),
    });
}

fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| {
        let src = source_for(FALLBACK).unwrap_or("{}");
        Catalog {
            active: parse(src),
            fallback: parse(src),
        }
    })
}

/// Translates `key`.
pub fn tr(key: &'static str) -> &'static str {
    let cat = catalog();
    cat.active
        .get(key)
        .or_else(|| cat.fallback.get(key))
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
        let en = parse(source_for(FALLBACK).expect("en locale"));
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
