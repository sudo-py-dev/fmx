//! Colorful artwork for categories and tools: each gets a filled icon and a hue.

use eframe::egui::Color32;

use crate::catalog::{AudioFormat, Category, Device, ImageFormat, Tool, VideoFormat};

/// A filled Phosphor glyph and the hue it is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Art {
    pub glyph: &'static str,
    pub hue: Color32,
}

/// A vivid palette that reads well on both light and dark backgrounds.
pub mod hue {
    use eframe::egui::Color32;

    pub const BLUE: Color32 = Color32::from_rgb(59, 130, 246);
    pub const INDIGO: Color32 = Color32::from_rgb(99, 102, 241);
    pub const VIOLET: Color32 = Color32::from_rgb(139, 92, 246);
    pub const PINK: Color32 = Color32::from_rgb(236, 72, 153);
    pub const ROSE: Color32 = Color32::from_rgb(244, 63, 94);
    pub const RED: Color32 = Color32::from_rgb(239, 68, 68);
    pub const ORANGE: Color32 = Color32::from_rgb(249, 115, 22);
    pub const AMBER: Color32 = Color32::from_rgb(245, 158, 11);
    pub const GREEN: Color32 = Color32::from_rgb(34, 197, 94);
    pub const EMERALD: Color32 = Color32::from_rgb(16, 185, 129);
    pub const TEAL: Color32 = Color32::from_rgb(20, 184, 166);
    pub const CYAN: Color32 = Color32::from_rgb(6, 182, 212);
    pub const SKY: Color32 = Color32::from_rgb(14, 165, 233);
    pub const SLATE: Color32 = Color32::from_rgb(100, 116, 139);
}

/// Filled-weight glyphs (same code points as the regular weight).
pub mod glyph {
    pub const ANDROID: &str = "\u{E008}";
    pub const APPLE: &str = "\u{E516}";
    pub const ARROW_CLOCKWISE: &str = "\u{E036}";
    pub const ARROWS_IN: &str = "\u{E09A}";
    pub const ARROWS_SPLIT: &str = "\u{ED3C}";
    pub const BOOK: &str = "\u{E8F2}";
    pub const CAPTIONS: &str = "\u{E1A4}";
    pub const CROP: &str = "\u{E1D4}";
    pub const DISC: &str = "\u{E564}";
    pub const DOWNLOAD: &str = "\u{E20C}";
    pub const ERASER: &str = "\u{E21E}";
    pub const EXCEL: &str = "\u{EB6C}";
    pub const FILE_ARCHIVE: &str = "\u{EB2A}";
    pub const FILE_AUDIO: &str = "\u{EA20}";
    pub const FILE_DOC: &str = "\u{EB1E}";
    pub const FILE_HTML: &str = "\u{EB38}";
    pub const FILE_IMAGE: &str = "\u{EA24}";
    pub const FILE_PDF: &str = "\u{E702}";
    pub const FILE_TEXT: &str = "\u{E23A}";
    pub const FILE_TXT: &str = "\u{EB35}";
    pub const FILE_VIDEO: &str = "\u{EA22}";
    pub const FILE_ZIP: &str = "\u{E958}";
    pub const FILM_SLATE: &str = "\u{E8C2}";
    pub const FILM_STRIP: &str = "\u{E792}";
    pub const FIRST_AID: &str = "\u{E570}";
    pub const GAME: &str = "\u{E26E}";
    pub const GAUGE: &str = "\u{E628}";
    pub const HARD_DRIVE: &str = "\u{E29E}";
    pub const IMAGE: &str = "\u{E2CA}";
    pub const IMAGES: &str = "\u{E836}";
    pub const LINK: &str = "\u{E2E2}";
    pub const MERGE: &str = "\u{E280}";
    pub const MOBILE: &str = "\u{E1E0}";
    pub const MUSIC_NOTES: &str = "\u{E340}";
    pub const PACKAGE: &str = "\u{E390}";
    pub const PIP: &str = "\u{E64C}";
    pub const RECORD: &str = "\u{E3EE}";
    pub const SCISSORS: &str = "\u{EAE0}";
    pub const SLIDERS: &str = "\u{E434}";
    pub const SPARKLE: &str = "\u{E6A2}";
    pub const SPEAKER: &str = "\u{E44A}";
    pub const SPEAKER_SLASH: &str = "\u{E45A}";
    pub const STACK: &str = "\u{E466}";
    pub const SUBTITLES: &str = "\u{E1A8}";
    pub const TABLET: &str = "\u{E1E6}";
    pub const TV: &str = "\u{E754}";
    pub const VINYL: &str = "\u{ECAC}";
    pub const WAVEFORM: &str = "\u{E802}";
    pub const WHATSAPP: &str = "\u{E5D0}";
    pub const WORD: &str = "\u{EB6A}";
    pub const WRENCH: &str = "\u{E5D4}";
    pub const YOUTUBE: &str = "\u{E4FC}";

    /// Every glyph above, for the font-coverage test.
    #[cfg(test)]
    pub const ALL: [&str; 52] = [
        ANDROID,
        APPLE,
        ARROW_CLOCKWISE,
        ARROWS_IN,
        ARROWS_SPLIT,
        BOOK,
        CAPTIONS,
        CROP,
        DISC,
        DOWNLOAD,
        ERASER,
        EXCEL,
        FILE_ARCHIVE,
        FILE_AUDIO,
        FILE_DOC,
        FILE_HTML,
        FILE_IMAGE,
        FILE_PDF,
        FILE_TEXT,
        FILE_TXT,
        FILE_VIDEO,
        FILE_ZIP,
        FILM_SLATE,
        FILM_STRIP,
        FIRST_AID,
        GAME,
        GAUGE,
        HARD_DRIVE,
        IMAGE,
        IMAGES,
        LINK,
        MERGE,
        MOBILE,
        MUSIC_NOTES,
        PACKAGE,
        PIP,
        RECORD,
        SCISSORS,
        SLIDERS,
        SPARKLE,
        SPEAKER,
        SPEAKER_SLASH,
        STACK,
        SUBTITLES,
        TABLET,
        TV,
        VINYL,
        WAVEFORM,
        WHATSAPP,
        WORD,
        WRENCH,
        YOUTUBE,
    ];
}

const fn art(glyph: &'static str, hue: Color32) -> Art {
    Art { glyph, hue }
}

pub fn category(c: Category) -> Art {
    match c {
        Category::Video => art(glyph::FILM_STRIP, hue::BLUE),
        Category::Devices => art(glyph::MOBILE, hue::TEAL),
        Category::Audio => art(glyph::MUSIC_NOTES, hue::PINK),
        Category::Image => art(glyph::IMAGE, hue::GREEN),
        Category::Document => art(glyph::FILE_TEXT, hue::ORANGE),
        Category::Rom => art(glyph::DISC, hue::VIOLET),
        Category::Utilities => art(glyph::WRENCH, hue::AMBER),
    }
}

fn video(f: VideoFormat) -> Art {
    use VideoFormat as V;
    match f {
        V::Gif => art(glyph::SPARKLE, hue::PINK),
        V::Webp => art(glyph::SPARKLE, hue::EMERALD),
        _ => art(
            glyph::FILE_VIDEO,
            match f {
                V::Mp4 => hue::BLUE,
                V::Mp4Hevc => hue::INDIGO,
                V::Mkv => hue::VIOLET,
                V::Webm => hue::GREEN,
                V::Mov => hue::SLATE,
                V::Avi => hue::ORANGE,
                V::Flv => hue::RED,
                V::Wmv => hue::SKY,
                V::Mpg | V::Vob => hue::AMBER,
                V::Ts => hue::TEAL,
                V::Ogv => hue::EMERALD,
                V::ThreeGp => hue::CYAN,
                V::Swf => hue::ROSE,
                V::Gif | V::Webp => hue::PINK,
            },
        ),
    }
}

fn audio(f: AudioFormat) -> Art {
    use AudioFormat as A;
    match f {
        A::Flac | A::Wav | A::Aiff | A::Wv | A::Ape => art(
            glyph::WAVEFORM,
            match f {
                A::Flac => hue::VIOLET,
                A::Wav => hue::BLUE,
                A::Aiff => hue::SLATE,
                A::Wv => hue::TEAL,
                _ => hue::INDIGO,
            },
        ),
        A::Mp3 => art(glyph::MUSIC_NOTES, hue::PINK),
        A::M4r => art(glyph::MOBILE, hue::ROSE),
        _ => art(
            glyph::FILE_AUDIO,
            match f {
                A::Aac | A::M4a => hue::ORANGE,
                A::Ogg | A::Opus => hue::EMERALD,
                A::Wma => hue::SKY,
                A::Ac3 | A::Mp2 => hue::AMBER,
                A::Mka => hue::VIOLET,
                _ => hue::CYAN,
            },
        ),
    }
}

fn image(f: ImageFormat) -> Art {
    use ImageFormat as I;
    art(
        glyph::FILE_IMAGE,
        match f {
            I::Png => hue::BLUE,
            I::Jpeg => hue::ORANGE,
            I::Webp => hue::GREEN,
            I::Bmp => hue::SLATE,
            I::Tiff => hue::INDIGO,
            I::Gif => hue::PINK,
            I::Ico => hue::AMBER,
            I::Tga => hue::CYAN,
            I::Qoi => hue::TEAL,
            I::Avif => hue::VIOLET,
            I::Jxl => hue::ROSE,
            I::Pcx => hue::SKY,
        },
    )
}

fn device(d: Device) -> Art {
    match d {
        Device::Iphone => art(glyph::APPLE, hue::SLATE),
        Device::Ipad => art(glyph::TABLET, hue::SLATE),
        Device::AndroidPhone => art(glyph::ANDROID, hue::GREEN),
        Device::AndroidTablet => art(glyph::TABLET, hue::GREEN),
        Device::Psp | Device::PsVita => art(glyph::GAME, hue::INDIGO),
        Device::Xbox => art(glyph::GAME, hue::EMERALD),
        Device::SmartTv => art(glyph::TV, hue::SKY),
        Device::Youtube => art(glyph::YOUTUBE, hue::RED),
        Device::Whatsapp => art(glyph::WHATSAPP, hue::GREEN),
    }
}

pub fn tool(t: Tool) -> Art {
    match t {
        Tool::ToVideo(f) => video(f),
        Tool::ToAudio(f) => audio(f),
        Tool::ToImage(f) => image(f),
        Tool::ToDevice(d) => device(d),
        Tool::JoinImages => art(glyph::STACK, hue::GREEN),
        Tool::ImagesToPdf => art(glyph::FILE_PDF, hue::RED),
        Tool::PdfToImages => art(glyph::IMAGES, hue::GREEN),
        Tool::PdfToText => art(glyph::FILE_TXT, hue::SLATE),
        Tool::PdfToHtml => art(glyph::FILE_HTML, hue::ORANGE),
        Tool::PdfToWord => art(glyph::WORD, hue::BLUE),
        Tool::PdfToExcel => art(glyph::EXCEL, hue::EMERALD),
        Tool::OfficeToPdf => art(glyph::FILE_DOC, hue::ROSE),
        Tool::PdfMerge => art(glyph::STACK, hue::RED),
        Tool::PdfSplit => art(glyph::ARROWS_SPLIT, hue::ORANGE),
        Tool::PdfCompress => art(glyph::ARROWS_IN, hue::AMBER),
        Tool::ToEbook(_) => art(glyph::BOOK, hue::VIOLET),
        Tool::DvdToVideo => art(glyph::DISC, hue::VIOLET),
        Tool::BluRayToVideo => art(glyph::DISC, hue::SKY),
        Tool::CdToAudio => art(glyph::VINYL, hue::PINK),
        Tool::DiscToIso => art(glyph::HARD_DRIVE, hue::INDIGO),
        Tool::FolderToIso => art(glyph::PACKAGE, hue::AMBER),
        Tool::IsoToCso => art(glyph::FILE_ZIP, hue::TEAL),
        Tool::CsoToIso => art(glyph::FILE_ARCHIVE, hue::TEAL),
        Tool::JoinVideo => art(glyph::LINK, hue::BLUE),
        Tool::JoinAudio => art(glyph::LINK, hue::PINK),
        Tool::Mux => art(glyph::MERGE, hue::VIOLET),
        Tool::AudioMix => art(glyph::SLIDERS, hue::PINK),
        Tool::VideoMix => art(glyph::PIP, hue::INDIGO),
        Tool::Trim => art(glyph::SCISSORS, hue::ORANGE),
        Tool::Split => art(glyph::ARROWS_SPLIT, hue::AMBER),
        Tool::Crop => art(glyph::CROP, hue::GREEN),
        Tool::Compress => art(glyph::ARROWS_IN, hue::TEAL),
        Tool::RemoveAudio => art(glyph::SPEAKER_SLASH, hue::ROSE),
        Tool::Delogo => art(glyph::ERASER, hue::CYAN),
        Tool::AddSubtitles => art(glyph::CAPTIONS, hue::SKY),
        Tool::ExtractSubtitles => art(glyph::SUBTITLES, hue::SKY),
        Tool::ExtractFrames => art(glyph::IMAGES, hue::GREEN),
        Tool::Rotate => art(glyph::ARROW_CLOCKWISE, hue::INDIGO),
        Tool::Speed => art(glyph::GAUGE, hue::ORANGE),
        Tool::Volume => art(glyph::SPEAKER, hue::PINK),
        Tool::Normalize => art(glyph::WAVEFORM, hue::VIOLET),
        Tool::Repair => art(glyph::FIRST_AID, hue::RED),
        Tool::Download => art(glyph::DOWNLOAD, hue::BLUE),
        Tool::Record => art(glyph::RECORD, hue::RED),
    }
}

/// The brand mark shown in the toolbar.
pub const LOGO: Art = art(glyph::FILM_SLATE, hue::BLUE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_art_in_a_vivid_hue() {
        for c in Category::ALL {
            for t in c.tools() {
                let a = tool(t);
                assert!(!a.glyph.is_empty());
                assert_ne!(a.hue, Color32::BLACK);
            }
        }
    }
}
