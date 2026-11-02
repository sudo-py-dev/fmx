//! Validate artwork before queueing and again before execution.
use crate::{i18n::tr, plan::PlanError};
use std::path::Path;

const MAX_COVER_BYTES: u64 = 20 * 1024 * 1024;
const MAX_COVER_PIXELS: u64 = 16_000_000;

pub fn validate_cover(path: &Path) -> Result<(), PlanError> {
    let invalid = || PlanError::InvalidValue(tr("tags.invalid_cover").to_owned());
    let metadata = std::fs::metadata(path).map_err(|_| invalid())?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_COVER_BYTES {
        return Err(invalid());
    }
    let reader = image::ImageReader::open(path)
        .map_err(|_| invalid())?
        .with_guessed_format()
        .map_err(|_| invalid())?;
    if !matches!(
        reader.format(),
        Some(image::ImageFormat::Jpeg | image::ImageFormat::Png)
    ) {
        return Err(invalid());
    }
    let (w, h) = reader.into_dimensions().map_err(|_| invalid())?;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || u64::from(w) * u64::from(h) > MAX_COVER_PIXELS {
        return Err(invalid());
    }
    Ok(())
}

/// Metadata scopes refer to the output's first audio stream and attached pictures,
/// never to a caller-provided command-line stream specifier.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum TagScope {
    #[default]
    File,
    Audio,
    Cover(u32),
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TagField {
    pub scope: TagScope,
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub write: bool,
}

pub const MAX_EXTRA_TAGS: usize = 128;
pub const MAX_EXTRA_CHARS: usize = 4096;

pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key == key.trim()
        && key.chars().count() <= 128
        && key
            .chars()
            .all(|c| c.is_alphanumeric() || "_- ./:".contains(c))
}

pub fn display_value(value: &str) -> String {
    value.chars().filter(|c| (!c.is_control() || matches!(*c, '\n' | '\r' | '\t'))
        && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}'))
        .take(MAX_EXTRA_CHARS).collect()
}

pub fn valid_fields(fields: &[TagField], primary_scopes: &[TagScope; 3]) -> bool {
    use std::collections::HashSet;
    if fields.len() > MAX_EXTRA_TAGS
        || primary_scopes
            .iter()
            .any(|scope| matches!(scope, TagScope::Cover(_)))
    {
        return false;
    }
    let mut seen: HashSet<_> = ["title", "artist", "album"]
        .into_iter()
        .zip(primary_scopes.iter().copied())
        .map(|(key, scope)| (scope, key.to_owned()))
        .collect();
    let mut bytes = 0usize;
    for field in fields {
        if (field.write && generated_key(&field.key))
            || !valid_key(&field.key)
            || field.value.chars().count() > MAX_EXTRA_CHARS
            || field
                .value
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
            || matches!(field.scope, TagScope::Cover(n) if n >= 128)
            || !seen.insert((field.scope, field.key.to_ascii_lowercase()))
        {
            return false;
        }
        if field.write {
            bytes = bytes.saturating_add(field.key.len() + field.value.len());
        }
    }
    bytes <= 16 * 1024
}

pub fn generated_key(key: &str) -> bool {
    [
        "encoder",
        "major_brand",
        "minor_version",
        "compatible_brands",
        "handler_name",
        "vendor_id",
        "duration",
    ]
    .iter()
    .any(|name| key.eq_ignore_ascii_case(name))
}

pub fn metadata_flag(scope: TagScope) -> String {
    match scope {
        TagScope::File => "-metadata".into(),
        TagScope::Audio => "-metadata:s:a:0".into(),
        TagScope::Cover(index) => format!("-metadata:s:v:{index}"),
    }
}

fn value_at<'a>(tags: &'a crate::probe::AudioTags, scope: TagScope, key: &str) -> Option<&'a str> {
    for (index, (name, value)) in [
        ("title", &tags.title),
        ("artist", &tags.artist),
        ("album", &tags.album),
    ]
    .into_iter()
    .enumerate()
    {
        if key.eq_ignore_ascii_case(name) && tags.primary_scopes[index] == scope {
            return Some(value);
        }
    }
    tags.extra
        .iter()
        .find(|f| f.scope == scope && f.key.eq_ignore_ascii_case(key))
        .map(|f| f.value.as_str())
}

/// Verify the staged file before it is published. Format-specific limitations must
/// not silently discard an explicit edit or other descriptive source metadata.
pub fn verify_saved(
    source: &crate::probe::MediaInfo,
    saved: &crate::probe::MediaInfo,
    o: &crate::options::MediaToolOptions,
) -> Result<(), PlanError> {
    let actual = saved.tags.as_ref().ok_or(PlanError::MetadataVerification)?;
    let covers = saved.streams.iter().filter(|s| s.attached_picture).count();
    let expected_covers = if o.remove_cover {
        0
    } else if o.cover.is_some() {
        1
    } else {
        source.streams.iter().filter(|s| s.attached_picture).count()
    };
    if covers != expected_covers {
        return Err(PlanError::MetadataNotSaved(tr("media.cover").into()));
    }
    let mut edits = Vec::new();
    for (index, (key, value)) in [
        ("title", &o.title),
        ("artist", &o.artist),
        ("album", &o.album),
    ]
    .into_iter()
    .enumerate()
    {
        if if o.tag_explicit {
            o.tag_write[index]
        } else {
            !value.trim().is_empty()
        } {
            edits.push((o.tag_scopes[index], key, value.as_str()));
        }
    }
    edits.extend(
        o.extra_tags
            .iter()
            .filter(|field| field.write)
            .map(|field| (field.scope, field.key.as_str(), field.value.as_str())),
    );
    let check = |scope, key: &str, expected: &str| {
        let value = value_at(actual, scope, key).unwrap_or("");
        if value == expected {
            Ok(())
        } else {
            Err(PlanError::MetadataNotSaved(key.into()))
        }
    };
    for &(scope, key, value) in &edits {
        if matches!(scope, TagScope::Cover(n) if n as usize >= covers) {
            continue;
        }
        check(scope, key, value)?;
    }
    if let Some(original) = &source.tags {
        let primary = [
            ("title", &original.title),
            ("artist", &original.artist),
            ("album", &original.album),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (key, value))| (original.primary_scopes[index], key, value.as_str()));
        let extra = original
            .extra
            .iter()
            .map(|field| (field.scope, field.key.as_str(), field.value.as_str()));
        for (scope, key, value) in primary.chain(extra) {
            if value.is_empty()
                || edits
                    .iter()
                    .any(|(s, k, _)| *s == scope && k.eq_ignore_ascii_case(key))
            {
                continue;
            }
            if matches!(scope, TagScope::Cover(_)) && (o.remove_cover || o.cover.is_some()) {
                continue;
            }
            // These are generated by the encoder/muxer, rather than descriptive tags.
            if [
                "encoder",
                "major_brand",
                "minor_version",
                "compatible_brands",
                "handler_name",
                "vendor_id",
                "creation_time",
                "duration",
            ]
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name))
                || (scope == TagScope::Audio
                    && key.eq_ignore_ascii_case("language")
                    && value == "und")
            {
                continue;
            }
            check(scope, key, value)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod field_tests {
    use super::*;
    #[test]
    fn tag_names_values_duplicates_and_scope_indices_are_bounded() {
        let primary = [TagScope::File; 3];
        let field = |key: &str| TagField {
            key: key.into(),
            value: "https://example.invalid/?x=1\ncomment".into(),
            write: true,
            ..Default::default()
        };
        assert!(valid_fields(&[field("purl")], &primary));
        for key in ["", "a=b", "bad\0key", "bad\nkey", "\u{202e}title"] {
            assert!(!valid_fields(&[field(key)], &primary));
        }
        assert!(!valid_fields(
            &[field("comment"), field("COMMENT")],
            &primary
        ));
        assert!(!valid_fields(&[field("TITLE")], &primary));
        assert!(!valid_fields(
            &[TagField {
                scope: TagScope::Cover(u32::MAX),
                ..field("title")
            }],
            &primary
        ));
        assert!(!valid_fields(
            &[TagField {
                value: "x".repeat(MAX_EXTRA_CHARS + 1),
                ..field("comment")
            }],
            &primary
        ));
        assert!(!valid_fields(
            &vec![field("comment"); MAX_EXTRA_TAGS + 1],
            &primary
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cover_validation_checks_content_size_and_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let cover = dir.path().join("cover.png");
        image::RgbImage::new(32, 32).save(&cover).unwrap();
        assert!(validate_cover(&cover).is_ok());
        std::fs::write(&cover, b"<svg><script>bad</script></svg>").unwrap();
        assert!(validate_cover(&cover).is_err());
        assert!(validate_cover(dir.path()).is_err());
        let oversized = std::fs::File::create(&cover).unwrap();
        oversized.set_len(MAX_COVER_BYTES + 1).unwrap();
        assert!(validate_cover(&cover).is_err());
        // A modest compressed image can still ask the decoder for too many pixels.
        image::RgbImage::new(4001, 4000).save(&cover).unwrap();
        assert!(validate_cover(&cover).is_err());
    }
}
