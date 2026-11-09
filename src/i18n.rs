//! Tiny translation layer: every user-facing string goes through [`tr`] / [`trf`].
//!
//! Locales are JSON maps embedded at compile time. The active locale is picked
//! from the environment once at startup; missing keys fall back to English,
//! then to the key itself so a gap is visible instead of silent.
//!
//! Right-to-left languages (see [`is_rtl`]) get each string prefixed with a
//! RIGHT-TO-LEFT MARK, so text that starts with a file format, number or icon
//! still reads right to left, and their arrows point the other way.

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
    ("he", include_str!("../locales/he.json")),
    ("ru", include_str!("../locales/ru.json")),
];
/// Languages written right to left.
const RTL_LANGUAGES: &[&str] = &["he"];
/// Sets the base direction of a right-to-left string (Unicode U+200F).
pub const RTL_MARK: char = '\u{200F}';
const FALLBACK: usize = 0;

static CATALOGS: OnceLock<Vec<HashMap<String, String>>> = OnceLock::new();
static ACTIVE: AtomicUsize = AtomicUsize::new(FALLBACK);

/// egui's built-in fonts have no U+2192 arrow, so it is drawn from the icon font.
const ARROW: char = '→';

pub(crate) fn parse(src: &str, rtl: bool) -> HashMap<String, String> {
    // The files are compiled in, so a parse failure is a build-time bug caught by tests.
    let map: HashMap<String, String> = serde_json::from_str(src).unwrap_or_default();
    map.into_iter()
        .map(|(k, v)| {
            let v = if rtl {
                format!("{RTL_MARK}{}", v.replace(ARROW, crate::icons::ARROW_LEFT))
            } else {
                v.replace(ARROW, crate::icons::ARROW_RIGHT)
            };
            (k, v)
        })
        .collect()
}

fn catalogs() -> &'static [HashMap<String, String>] {
    CATALOGS.get_or_init(|| {
        LOCALES
            .iter()
            .map(|(code, src)| parse(src, RTL_LANGUAGES.contains(code)))
            .collect()
    })
}

/// The active language's code, such as `en`.
pub fn active_language() -> &'static str {
    LOCALES[ACTIVE.load(Ordering::Relaxed)].0
}

/// Whether the active language is written right to left.
pub fn is_rtl() -> bool {
    RTL_LANGUAGES.contains(&LOCALES[ACTIVE.load(Ordering::Relaxed)].0)
}

/// Prefix for text built from parts (an icon and a label, say), so it reads
/// in the UI's direction even when its first part is an icon or a number.
pub fn direction_mark() -> &'static str {
    if is_rtl() { "\u{200F}" } else { "" }
}

/// Extracts the language code from a POSIX locale such as `de_DE.UTF-8`.
fn language_code(locale: &str) -> &str {
    locale.split(['_', '.', '@', '-']).next().unwrap_or("en")
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .find(|v| !v.is_empty())
        .map(|v| language_code(&v).to_owned())
        .unwrap_or_else(|| "en".to_owned())
}

// Match the Windows display language, rather than its date/number locale.
#[cfg(windows)]
fn system_language() -> String {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // SAFETY: this Windows API takes no pointers and is available on all supported versions.
    let primary = unsafe { GetUserDefaultUILanguage() } & 0x03ff;
    match primary {
        0x07 => "de",
        0x0a => "es",
        0x0c => "fr",
        0x0d => "he",
        0x19 => "ru",
        _ => "en",
    }
    .to_owned()
}

#[cfg(target_os = "macos")]
fn system_language() -> String {
    use std::ffi::{c_char, c_void};
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> *const c_void;
        fn CFArrayGetCount(array: *const c_void) -> isize;
        fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        fn CFStringGetCString(
            string: *const c_void,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(value: *const c_void);
    }
    // SAFETY: the copied array is checked, indexed only when nonempty, and released
    // once. Its first value is a CFString; the UTF-8 buffer remains valid for the call.
    unsafe {
        let languages = CFLocaleCopyPreferredLanguages();
        if languages.is_null() {
            return "en".into();
        }
        let mut buffer = [0_u8; 256];
        let ok = CFArrayGetCount(languages) > 0
            && CFStringGetCString(
                CFArrayGetValueAtIndex(languages, 0),
                buffer.as_mut_ptr().cast(),
                buffer.len() as isize,
                0x08000100,
            ) != 0;
        CFRelease(languages);
        if ok {
            let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
            return String::from_utf8_lossy(&buffer[..end]).into_owned();
        }
    }
    "en".into()
}

/// Resolve regional variants and unknown languages without changing global state.
fn language_index(locale: &str) -> usize {
    let code = language_code(locale.trim());
    let code = if code.eq_ignore_ascii_case("iw") {
        "he"
    } else {
        code
    };
    LOCALES
        .iter()
        .position(|(c, _)| c.eq_ignore_ascii_case(code))
        .unwrap_or(FALLBACK)
}

/// Available languages as (code, native name).
pub fn languages() -> Vec<(&'static str, &'static str)> {
    LOCALES
        .iter()
        .zip(catalogs())
        .map(|((code, _), map)| {
            let name = map.get("language.name").map_or(*code, String::as_str);
            (*code, name.trim_start_matches(RTL_MARK))
        })
        .collect()
}

/// Switches the UI language; `None` follows the system locale. Unknown
/// codes fall back to English.
pub fn set_language(code: Option<&str>) {
    let wanted = code.map_or_else(system_language, str::to_owned);
    let index = language_index(&wanted);
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

    #[test]
    fn russian_translation_covers_every_english_key() {
        let en = &catalogs()[FALLBACK];
        let ru_index = LOCALES.iter().position(|(code, _)| *code == "ru").unwrap();
        let ru = &catalogs()[ru_index];
        let missing: Vec<_> = en.keys().filter(|key| !ru.contains_key(*key)).collect();
        assert!(
            missing.is_empty(),
            "missing Russian translations: {missing:?}"
        );
        assert!(ru.values().all(|text| !text.trim().is_empty()));
        assert_eq!(ru.get("language.name").map(String::as_str), Some("Русский"));
    }

    #[test]
    fn collage_translations_cover_all_languages() {
        let en = &catalogs()[FALLBACK];
        for ((code, _), map) in LOCALES.iter().zip(catalogs()).skip(1) {
            for key in en.keys().filter(|key| {
                key.starts_with("collage.")
                    || matches!(key.as_str(), "join.grid2" | "join.grid3" | "join.featured")
            }) {
                assert!(map.contains_key(key), "{code}: missing {key}");
            }
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
    fn regional_languages_and_english_fallback() {
        for (locale, expected) in [
            ("he-IL", "he"),
            ("iw_IL", "he"),
            ("FR_ca.UTF-8", "fr"),
            ("ru-RU", "ru"),
            ("ja-JP", "en"),
            ("C", "en"),
            ("", "en"),
        ] {
            assert_eq!(LOCALES[language_index(locale)].0, expected);
        }
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
        let mut sources = Vec::new();
        collect_rust_sources(std::path::Path::new(dir), &mut sources);
        for path in sources {
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

    fn collect_rust_sources(dir: &std::path::Path, sources: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("source directory") {
            let entry = entry.expect("source entry");
            let kind = entry.file_type().expect("source type");
            let path = entry.path();
            if kind.is_dir() {
                collect_rust_sources(&path, sources);
            } else if kind.is_file()
                && path.extension().is_some_and(|ext| ext == "rs")
                && !path.ends_with("i18n.rs")
            {
                sources.push(path);
            }
        }
    }

    #[test]
    fn source_scan_handles_nested_directories_and_non_rust_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        let source = dir.path().join("nested/module.rs");
        std::fs::write(&source, "").unwrap();
        std::fs::write(dir.path().join("notes.txt"), [0xff]).unwrap();
        let mut sources = Vec::new();
        collect_rust_sources(dir.path(), &mut sources);
        assert_eq!(sources, [source]);
    }
}
