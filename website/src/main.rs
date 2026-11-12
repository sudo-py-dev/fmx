//! Builds the static FMX website into `dist`.
//!
//! English lives in `static/index.html`. Every other language is rendered
//! from `static/translations.js` into its own static page (`/es/`, `/fr/`, …)
//! so search engines can index each one, with `hreflang` links between them.

use std::{fs, io, path::Path};

const SITE: &str = "https://fmx-dun.vercel.app";

/// A page language: code, Open Graph locale, native name, text direction.
struct Language {
    code: &'static str,
    locale: &'static str,
    name: &'static str,
    rtl: bool,
}

const LANGUAGES: [Language; 6] = [
    Language { code: "en", locale: "en_US", name: "English", rtl: false },
    Language { code: "es", locale: "es_ES", name: "Español", rtl: false },
    Language { code: "fr", locale: "fr_FR", name: "Français", rtl: false },
    Language { code: "de", locale: "de_DE", name: "Deutsch", rtl: false },
    Language { code: "he", locale: "he_IL", name: "עברית", rtl: true },
    Language { code: "ru", locale: "ru_RU", name: "Русский", rtl: false },
];

/// Keys whose translations contain trusted inline markup (`<br>`, `<em>`, `<b>`).
const MARKUP_KEYS: [&str; 5] = ["heroTitle", "mediaTypes", "featureTitle", "featureIntro", "faqTitle"];

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn path_for(code: &str) -> String {
    if code == "en" { "/".to_owned() } else { format!("/{code}/") }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Returns the text between `start` and the next `end` after it.
fn between<'a>(source: &'a str, start: &str, end: &str) -> io::Result<&'a str> {
    let from = source.find(start).ok_or_else(|| invalid(format!("missing {start:?}")))? + start.len();
    let length = source[from..].find(end).ok_or_else(|| invalid(format!("unterminated {start:?}")))?;
    Ok(&source[from..from + length])
}

/// Reads the translation catalog for `code` as (key, value) pairs.
fn catalog(script: &str, code: &str) -> io::Result<Vec<(String, String)>> {
    let keys: Vec<&str> = between(script, "const translationKeys = `", "`")?.split('|').collect();
    let block = between(script, &format!("\n  {code}: [\n"), "\n  ]")?;
    // Strings are single-quoted and contain no escapes; reject anything else.
    if block.contains('\\') {
        return Err(invalid(format!("{code}: escapes are not supported")));
    }
    let values: Vec<&str> = block.split('\'').skip(1).step_by(2).collect();
    if values.len() != keys.len() {
        return Err(invalid(format!("{code}: {} values for {} keys", values.len(), keys.len())));
    }
    Ok(keys.iter().zip(values).map(|(key, value)| ((*key).to_owned(), value.to_owned())).collect())
}

fn lookup<'a>(messages: &'a [(String, String)], key: &str) -> io::Result<&'a str> {
    messages
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| invalid(format!("no translation for {key:?}")))
}

/// Replaces the content of every `data-i18n` element.
fn translate_text(html: &str, messages: &[(String, String)]) -> io::Result<String> {
    const MARK: &str = "data-i18n=\"";
    let mut output = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(found) = rest.find(MARK) {
        let key_start = found + MARK.len();
        let key_length = rest[key_start..].find('"').ok_or_else(|| invalid("unterminated data-i18n".into()))?;
        let key = &rest[key_start..key_start + key_length];
        let tag_start = rest[..found].rfind('<').ok_or_else(|| invalid(format!("{key}: no tag")))?;
        let name: String = rest[tag_start + 1..].chars().take_while(char::is_ascii_alphanumeric).collect();
        let content_start = key_start + rest[key_start..].find('>').ok_or_else(|| invalid(format!("{key}: open tag")))? + 1;
        let close = format!("</{name}>");
        let content_end = content_start
            + rest[content_start..].find(&close).ok_or_else(|| invalid(format!("{key}: no {close}")))?;
        let content = &rest[content_start..content_end];
        if content.contains(&format!("<{name}")) || content.contains(MARK) {
            return Err(invalid(format!("{key}: nested element is not supported")));
        }
        let value = lookup(messages, key)?;
        output.push_str(&rest[..content_start]);
        if MARKUP_KEYS.contains(&key) { output.push_str(value) } else { output.push_str(&escape(value)) }
        rest = &rest[content_end..];
    }
    output.push_str(rest);
    Ok(output)
}

/// Sets every attribute named by `data-i18n-attr="key:attribute"`.
fn translate_attributes(html: &str, messages: &[(String, String)]) -> io::Result<String> {
    const MARK: &str = "data-i18n-attr=\"";
    let mut output = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(found) = rest.find(MARK) {
        let tag_start = rest[..found].rfind('<').ok_or_else(|| invalid("data-i18n-attr outside a tag".into()))?;
        let tag_end = found + rest[found..].find('>').ok_or_else(|| invalid("unterminated tag".into()))?;
        let mut tag = rest[tag_start..tag_end].to_owned();
        let spec = between(&tag, MARK, "\"")?.to_owned();
        let (key, attribute) = spec.split_once(':').ok_or_else(|| invalid(format!("bad spec {spec:?}")))?;
        let prefix = format!(" {attribute}=\"");
        let value_start = tag.find(&prefix).ok_or_else(|| invalid(format!("{key}: no {attribute}")))? + prefix.len();
        let value_end = value_start + tag[value_start..].find('"').ok_or_else(|| invalid(format!("{key}: value")))?;
        tag.replace_range(value_start..value_end, &escape(lookup(messages, key)?));
        output.push_str(&rest[..tag_start]);
        output.push_str(&tag);
        rest = &rest[tag_end..];
    }
    output.push_str(rest);
    Ok(output)
}

/// Replaces `from` with `to`, requiring exactly `count` occurrences.
fn replace_exact(html: &str, from: &str, to: &str, count: usize) -> io::Result<String> {
    let found = html.matches(from).count();
    if found != count {
        return Err(invalid(format!("expected {count} of {from:?}, found {found}")));
    }
    Ok(html.replace(from, to))
}

fn alternates() -> String {
    let mut links: Vec<String> = LANGUAGES
        .iter()
        .map(|language| format!("<link rel=\"alternate\" hreflang=\"{}\" href=\"{SITE}{}\">", language.code, path_for(language.code)))
        .collect();
    links.push(format!("<link rel=\"alternate\" hreflang=\"x-default\" href=\"{SITE}/\">"));
    links.join("\n")
}

fn switcher(current: &str) -> String {
    LANGUAGES
        .iter()
        .map(|language| {
            let state = if language.code == current { " aria-current=\"page\"" } else { "" };
            format!(
                "<a href=\"{}\" hreflang=\"{code}\" lang=\"{code}\" data-language=\"{code}\"{state}>{}</a>",
                path_for(language.code),
                language.name,
                code = language.code,
            )
        })
        .collect()
}

fn render(template: &str, language: &Language, script: &str) -> io::Result<String> {
    let mut html = replace_exact(template, "<!--alternates-->", &alternates(), 1)?;
    html = replace_exact(&html, "<!--languages-->", &switcher(language.code), 1)?;
    if language.code == "en" {
        return Ok(html);
    }
    let messages = catalog(script, language.code)?;
    html = translate_attributes(&translate_text(&html, &messages)?, &messages)?;
    let direction = if language.rtl { "rtl" } else { "ltr" };
    html = replace_exact(&html, "<html lang=\"en\">", &format!("<html lang=\"{}\" dir=\"{direction}\">", language.code), 1)?;
    html = replace_exact(&html, "content=\"en_US\"", &format!("content=\"{}\"", language.locale), 1)?;
    let url = format!("{SITE}{}", path_for(language.code));
    html = replace_exact(&html, &format!("<link rel=\"canonical\" href=\"{SITE}/\">"), &format!("<link rel=\"canonical\" href=\"{url}\">"), 1)?;
    html = replace_exact(&html, &format!("<meta property=\"og:url\" content=\"{SITE}/\">"), &format!("<meta property=\"og:url\" content=\"{url}\">"), 1)?;
    replace_exact(&html, "<a class=\"brand\" href=\"/\"", &format!("<a class=\"brand\" href=\"{}\"", path_for(language.code)), 2)
}

fn sitemap() -> String {
    let alternates: String = LANGUAGES
        .iter()
        .map(|language| format!("\n    <xhtml:link rel=\"alternate\" hreflang=\"{}\" href=\"{SITE}{}\"/>", language.code, path_for(language.code)))
        .chain([format!("\n    <xhtml:link rel=\"alternate\" hreflang=\"x-default\" href=\"{SITE}/\"/>")])
        .collect();
    let urls: String = LANGUAGES
        .iter()
        .map(|language| format!("  <url>\n    <loc>{SITE}{}</loc>{alternates}\n  </url>\n", path_for(language.code)))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\" xmlns:xhtml=\"http://www.w3.org/1999/xhtml\">\n{urls}</urlset>\n"
    )
}

fn main() -> io::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dist = root.join("dist");
    copy_tree(&root.join("static"), &dist)?;
    let template = fs::read_to_string(root.join("static/index.html"))?;
    let script = fs::read_to_string(root.join("static/translations.js"))?;
    for language in &LANGUAGES {
        let directory = if language.code == "en" { dist.clone() } else { dist.join(language.code) };
        fs::create_dir_all(&directory)?;
        fs::write(directory.join("index.html"), render(&template, language, &script)?)?;
    }
    fs::write(dist.join("sitemap.xml"), sitemap())?;
    println!("Built FMX download website in {}", dist.display());
    Ok(())
}
