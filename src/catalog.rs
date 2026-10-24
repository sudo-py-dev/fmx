//! The tool catalog: every conversion and utility the app offers.

use serde::{Deserialize, Serialize};

use crate::externals::External;
use crate::i18n::tr;
use crate::icons;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Video,
    Audio,
    Image,
    Document,
    Rom,
    Utilities,
}

impl Category {
    pub const ALL: [Self; 6] = [
        Self::Video,
        Self::Audio,
        Self::Image,
        Self::Document,
        Self::Rom,
        Self::Utilities,
    ];

    pub fn label(self) -> String {
        let (icon, key) = match self {
            Self::Video => (icons::FILM_STRIP, "category.video"),
            Self::Audio => (icons::MUSIC_NOTES, "category.audio"),
            Self::Image => (icons::IMAGE, "category.image"),
            Self::Document => (icons::FILE_TEXT, "category.document"),
            Self::Rom => (icons::DISC, "category.rom"),
            Self::Utilities => (icons::WRENCH, "category.utilities"),
        };
        icons::label(icon, tr(key))
    }

    pub fn tools(self) -> Vec<Tool> {
        match self {
            Self::Video => VideoFormat::ALL.into_iter().map(Tool::ToVideo).collect(),
            Self::Audio => AudioFormat::ALL.into_iter().map(Tool::ToAudio).collect(),
            Self::Image => ImageFormat::ALL.into_iter().map(Tool::ToImage).collect(),
            Self::Document => [
                Tool::ImagesToPdf,
                Tool::PdfToImages,
                Tool::PdfToText,
                Tool::PdfToHtml,
                Tool::PdfToWord,
                Tool::OfficeToPdf,
                Tool::PdfMerge,
                Tool::PdfSplit,
                Tool::PdfCompress,
            ]
            .into_iter()
            .chain(EbookFormat::ALL.into_iter().map(Tool::ToEbook))
            .collect(),
            Self::Rom => vec![
                Tool::DvdToVideo,
                Tool::CdToAudio,
                Tool::FolderToIso,
                Tool::IsoToCso,
                Tool::CsoToIso,
            ],
            Self::Utilities => vec![
                Tool::JoinVideo,
                Tool::JoinAudio,
                Tool::Mux,
                Tool::AudioMix,
                Tool::VideoMix,
                Tool::Trim,
                Tool::Split,
                Tool::Compress,
                Tool::RemoveAudio,
                Tool::Delogo,
                Tool::ExtractFrames,
                Tool::Rotate,
                Tool::Speed,
                Tool::Volume,
                Tool::Normalize,
                Tool::Repair,
                Tool::Download,
                Tool::Record,
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VideoFormat {
    Mp4,
    Mp4Hevc,
    Mkv,
    Webm,
    Mov,
    Avi,
    Flv,
    Wmv,
    Mpg,
    Vob,
    Ts,
    Ogv,
    ThreeGp,
    Swf,
    Gif,
    Webp,
}

impl VideoFormat {
    pub const ALL: [Self; 16] = [
        Self::Mp4,
        Self::Mp4Hevc,
        Self::Mkv,
        Self::Webm,
        Self::Mov,
        Self::Avi,
        Self::Flv,
        Self::Wmv,
        Self::Mpg,
        Self::Vob,
        Self::Ts,
        Self::Ogv,
        Self::ThreeGp,
        Self::Swf,
        Self::Gif,
        Self::Webp,
    ];

    pub fn ext(self) -> &'static str {
        match self {
            Self::Mp4 | Self::Mp4Hevc => "mp4",
            Self::Mkv => "mkv",
            Self::Webm => "webm",
            Self::Mov => "mov",
            Self::Avi => "avi",
            Self::Flv => "flv",
            Self::Wmv => "wmv",
            Self::Mpg => "mpg",
            Self::Vob => "vob",
            Self::Ts => "ts",
            Self::Ogv => "ogv",
            Self::ThreeGp => "3gp",
            Self::Swf => "swf",
            Self::Gif => "gif",
            Self::Webp => "webp",
        }
    }

    /// Format names are technical identifiers and are not translated.
    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Mp4Hevc => "MP4 (H.265)",
            Self::Mkv => "MKV",
            Self::Webm => "WebM",
            Self::Mov => "MOV",
            Self::Avi => "AVI",
            Self::Flv => "FLV",
            Self::Wmv => "WMV",
            Self::Mpg => "MPG",
            Self::Vob => "VOB (DVD)",
            Self::Ts => "TS",
            Self::Ogv => "OGV",
            Self::ThreeGp => "3GP",
            Self::Swf => "SWF",
            Self::Gif => "GIF",
            Self::Webp => "WebP (animated)",
        }
    }

    /// The format whose container matches a file extension, if any.
    pub fn from_ext(ext: &str) -> Option<Self> {
        let ext = ext.to_ascii_lowercase();
        match ext.as_str() {
            "m4v" => Some(Self::Mp4),
            "mpeg" => Some(Self::Mpg),
            "mts" | "m2ts" => Some(Self::Ts),
            _ => Self::ALL.into_iter().find(|f| f.ext() == ext),
        }
    }

    /// Animated-image outputs carry no audio.
    pub fn is_animation(self) -> bool {
        matches!(self, Self::Gif | Self::Webp)
    }

    /// Containers that accept a choice of modern codecs.
    pub fn encoder_choices(self) -> &'static [VideoEncoder] {
        use VideoEncoder::{Auto, Av1, H264, H265, Vp9};
        match self {
            Self::Mp4 | Self::Mkv => &[Auto, H264, H265, Av1],
            Self::Mov => &[Auto, H264, H265],
            Self::Webm => &[Auto, Vp9, Av1],
            _ => &[Auto],
        }
    }

    /// The codec used when the encoder choice is `Auto`.
    pub fn default_encoder(self) -> VideoEncoder {
        match self {
            Self::Mp4Hevc => VideoEncoder::H265,
            Self::Webm => VideoEncoder::Vp9,
            Self::Mp4 | Self::Mkv | Self::Mov | Self::Flv | Self::Ts | Self::ThreeGp => {
                VideoEncoder::H264
            }
            _ => VideoEncoder::Legacy,
        }
    }

    /// The ffmpeg encoder that must exist for this format to work.
    pub fn required_encoder(self) -> &'static str {
        match self {
            Self::Mp4 | Self::Mkv | Self::Mov | Self::Flv | Self::Ts | Self::ThreeGp => "libx264",
            Self::Mp4Hevc => "libx265",
            Self::Webm => "libvpx-vp9",
            Self::Avi => "mpeg4",
            Self::Wmv => "wmv2",
            Self::Mpg | Self::Vob => "mpeg2video",
            Self::Ogv => "libtheora",
            Self::Swf => "flv",
            Self::Gif => "gif",
            Self::Webp => "libwebp_anim",
        }
    }
}

/// Video codec family; hardware acceleration picks the concrete encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VideoEncoder {
    Auto,
    H264,
    H265,
    Av1,
    Vp9,
    /// The fixed codec of an older container (MPEG-4, WMV2, MPEG-2…).
    Legacy,
}

impl VideoEncoder {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => tr("encoder.auto"),
            Self::H264 => "H.264",
            Self::H265 => "H.265 / HEVC",
            Self::Av1 => "AV1",
            Self::Vp9 => "VP9",
            Self::Legacy => tr("encoder.auto"),
        }
    }

    pub fn supports_hardware(self) -> bool {
        matches!(self, Self::H264 | Self::H265 | Self::Av1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioFormat {
    Mp3,
    Aac,
    M4a,
    M4r,
    Flac,
    Wav,
    Ogg,
    Opus,
    Wma,
    Aiff,
    Ac3,
    Mp2,
    Wv,
    Mka,
    Mmf,
    Amr,
}

impl AudioFormat {
    pub const ALL: [Self; 16] = [
        Self::Mp3,
        Self::Aac,
        Self::M4a,
        Self::M4r,
        Self::Flac,
        Self::Wav,
        Self::Ogg,
        Self::Opus,
        Self::Wma,
        Self::Aiff,
        Self::Ac3,
        Self::Mp2,
        Self::Wv,
        Self::Mka,
        Self::Mmf,
        Self::Amr,
    ];

    pub fn ext(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Aac => "aac",
            Self::M4a => "m4a",
            Self::M4r => "m4r",
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Ogg => "ogg",
            Self::Opus => "opus",
            Self::Wma => "wma",
            Self::Aiff => "aiff",
            Self::Ac3 => "ac3",
            Self::Mp2 => "mp2",
            Self::Wv => "wv",
            Self::Mka => "mka",
            Self::Mmf => "mmf",
            Self::Amr => "amr",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mp3 => "MP3",
            Self::Aac => "AAC",
            Self::M4a => "M4A",
            Self::M4r => "M4R (iPhone ringtone)",
            Self::Flac => "FLAC",
            Self::Wav => "WAV",
            Self::Ogg => "OGG",
            Self::Opus => "Opus",
            Self::Wma => "WMA",
            Self::Aiff => "AIFF",
            Self::Ac3 => "AC3",
            Self::Mp2 => "MP2",
            Self::Wv => "WavPack",
            Self::Mka => "MKA",
            Self::Mmf => "MMF",
            Self::Amr => "AMR",
        }
    }

    pub fn required_encoder(self) -> &'static str {
        match self {
            Self::Mp3 => "libmp3lame",
            Self::Aac | Self::M4a | Self::M4r | Self::Mka => "aac",
            Self::Flac => "flac",
            Self::Wav => "pcm_s16le",
            Self::Aiff => "pcm_s16be",
            Self::Ogg => "libvorbis",
            Self::Opus => "libopus",
            Self::Wma => "wmav2",
            Self::Ac3 => "ac3",
            Self::Mp2 => "mp2",
            Self::Wv => "wavpack",
            Self::Mmf => "adpcm_yamaha",
            Self::Amr => "libopencore_amrnb",
        }
    }

    /// Lossless and PCM formats ignore bitrate settings.
    pub fn is_lossless(self) -> bool {
        matches!(self, Self::Flac | Self::Wav | Self::Aiff | Self::Wv)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
    Bmp,
    Tiff,
    Gif,
    Ico,
    Tga,
    Qoi,
}

impl ImageFormat {
    pub const ALL: [Self; 9] = [
        Self::Png,
        Self::Jpeg,
        Self::Webp,
        Self::Bmp,
        Self::Tiff,
        Self::Gif,
        Self::Ico,
        Self::Tga,
        Self::Qoi,
    ];

    pub fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Gif => "gif",
            Self::Ico => "ico",
            Self::Tga => "tga",
            Self::Qoi => "qoi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPG",
            Self::Webp => "WebP",
            Self::Bmp => "BMP",
            Self::Tiff => "TIFF",
            Self::Gif => "GIF",
            Self::Ico => "ICO",
            Self::Tga => "TGA",
            Self::Qoi => "QOI",
        }
    }

    pub fn codec(self) -> image::ImageFormat {
        match self {
            Self::Png => image::ImageFormat::Png,
            Self::Jpeg => image::ImageFormat::Jpeg,
            Self::Webp => image::ImageFormat::WebP,
            Self::Bmp => image::ImageFormat::Bmp,
            Self::Tiff => image::ImageFormat::Tiff,
            Self::Gif => image::ImageFormat::Gif,
            Self::Ico => image::ImageFormat::Ico,
            Self::Tga => image::ImageFormat::Tga,
            Self::Qoi => image::ImageFormat::Qoi,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EbookFormat {
    Epub,
    Mobi,
    Azw3,
    Fb2,
    Pdf,
    Txt,
}

impl EbookFormat {
    pub const ALL: [Self; 6] = [
        Self::Epub,
        Self::Mobi,
        Self::Azw3,
        Self::Fb2,
        Self::Pdf,
        Self::Txt,
    ];

    pub fn ext(self) -> &'static str {
        match self {
            Self::Epub => "epub",
            Self::Mobi => "mobi",
            Self::Azw3 => "azw3",
            Self::Fb2 => "fb2",
            Self::Pdf => "pdf",
            Self::Txt => "txt",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Epub => tr("tool.ebook_epub"),
            Self::Mobi => tr("tool.ebook_mobi"),
            Self::Azw3 => tr("tool.ebook_azw3"),
            Self::Fb2 => tr("tool.ebook_fb2"),
            Self::Pdf => tr("tool.ebook_pdf"),
            Self::Txt => tr("tool.ebook_txt"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tool {
    ToVideo(VideoFormat),
    ToAudio(AudioFormat),
    ToImage(ImageFormat),
    ImagesToPdf,
    PdfToImages,
    PdfToText,
    PdfToHtml,
    PdfToWord,
    OfficeToPdf,
    PdfMerge,
    PdfSplit,
    PdfCompress,
    ToEbook(EbookFormat),
    DvdToVideo,
    CdToAudio,
    FolderToIso,
    IsoToCso,
    CsoToIso,
    JoinVideo,
    JoinAudio,
    Mux,
    AudioMix,
    VideoMix,
    Trim,
    Split,
    Compress,
    RemoveAudio,
    Delogo,
    ExtractFrames,
    Rotate,
    Speed,
    Volume,
    Normalize,
    Repair,
    Download,
    Record,
}

/// How a tool groups its inputs into jobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputArity {
    /// One job per input.
    EachFile,
    /// One job consuming all inputs, needing at least this many.
    AllAtLeast(usize),
    /// One job consuming exactly this many inputs, in order.
    Exactly(usize),
    /// No inputs at all (screen recording).
    None,
}

/// What the user supplies as input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Media,
    Images,
    Pdf,
    Documents,
    Ebooks,
    /// A disc folder, image file, or drive.
    Disc,
    Folder,
    Iso,
    Cso,
    /// An optical drive device path.
    Device,
    Url,
    Capture,
}

impl InputKind {
    /// File extensions accepted by the file picker; empty = any file.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Media => &[
                "mp4", "m4v", "mkv", "webm", "mov", "avi", "flv", "wmv", "mpg", "mpeg", "ts",
                "mts", "m2ts", "ogv", "3gp", "gif", "vob", "rmvb", "rm", "asf", "swf", "mp3",
                "aac", "m4a", "flac", "wav", "ogg", "opus", "wma", "aiff", "aif", "ac3", "amr",
                "ape", "mka", "wv", "mp2", "mmf", "m4r",
            ],
            Self::Images => &[
                "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif", "ico", "tga", "qoi",
                "pnm", "ppm", "pgm", "pbm", "heic", "heif", "avif", "jxl",
            ],
            Self::Pdf => &["pdf"],
            Self::Documents => &[
                "doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "csv", "ppt", "pptx", "odp",
                "txt", "html",
            ],
            Self::Ebooks => &[
                "epub", "mobi", "azw3", "azw", "fb2", "pdf", "txt", "docx", "html", "lit", "pdb",
            ],
            Self::Disc | Self::Iso => &["iso"],
            Self::Cso => &["cso"],
            Self::Folder | Self::Device | Self::Url | Self::Capture => &[],
        }
    }

    /// Whether the user picks folders (or types a path) rather than files.
    pub fn picks_folders(self) -> bool {
        matches!(self, Self::Disc | Self::Folder)
    }
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::ToVideo(f) => f.label(),
            Self::ToAudio(f) => f.label(),
            Self::ToImage(f) => f.label(),
            Self::ToEbook(f) => f.label(),
            Self::ImagesToPdf => tr("tool.images_to_pdf"),
            Self::PdfToImages => tr("tool.pdf_to_images"),
            Self::PdfToText => tr("tool.pdf_to_text"),
            Self::PdfToHtml => tr("tool.pdf_to_html"),
            Self::PdfToWord => tr("tool.pdf_to_word"),
            Self::OfficeToPdf => tr("tool.office_to_pdf"),
            Self::PdfMerge => tr("tool.pdf_merge"),
            Self::PdfSplit => tr("tool.pdf_split"),
            Self::PdfCompress => tr("tool.pdf_compress"),
            Self::DvdToVideo => tr("tool.dvd_to_video"),
            Self::CdToAudio => tr("tool.cd_to_audio"),
            Self::FolderToIso => tr("tool.folder_to_iso"),
            Self::IsoToCso => tr("tool.iso_to_cso"),
            Self::CsoToIso => tr("tool.cso_to_iso"),
            Self::JoinVideo => tr("tool.join_video"),
            Self::JoinAudio => tr("tool.join_audio"),
            Self::Mux => tr("tool.mux"),
            Self::AudioMix => tr("tool.audio_mix"),
            Self::VideoMix => tr("tool.video_mix"),
            Self::Trim => tr("tool.trim"),
            Self::Split => tr("tool.split"),
            Self::Compress => tr("tool.compress"),
            Self::RemoveAudio => tr("tool.remove_audio"),
            Self::Delogo => tr("tool.delogo"),
            Self::ExtractFrames => tr("tool.extract_frames"),
            Self::Rotate => tr("tool.rotate"),
            Self::Speed => tr("tool.speed"),
            Self::Volume => tr("tool.volume"),
            Self::Normalize => tr("tool.normalize"),
            Self::Repair => tr("tool.repair"),
            Self::Download => tr("tool.download"),
            Self::Record => tr("tool.record"),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::ToVideo(_) => tr("desc.to_video"),
            Self::ToAudio(_) => tr("desc.to_audio"),
            Self::ToImage(_) => tr("desc.to_image"),
            Self::ToEbook(_) => tr("desc.to_ebook"),
            Self::ImagesToPdf => tr("desc.images_to_pdf"),
            Self::PdfToImages => tr("desc.pdf_to_images"),
            Self::PdfToText => tr("desc.pdf_to_text"),
            Self::PdfToHtml => tr("desc.pdf_to_html"),
            Self::PdfToWord => tr("desc.pdf_to_word"),
            Self::OfficeToPdf => tr("desc.office_to_pdf"),
            Self::PdfMerge => tr("desc.pdf_merge"),
            Self::PdfSplit => tr("desc.pdf_split"),
            Self::PdfCompress => tr("desc.pdf_compress"),
            Self::DvdToVideo => tr("desc.dvd_to_video"),
            Self::CdToAudio => tr("desc.cd_to_audio"),
            Self::FolderToIso => tr("desc.folder_to_iso"),
            Self::IsoToCso => tr("desc.iso_to_cso"),
            Self::CsoToIso => tr("desc.cso_to_iso"),
            Self::JoinVideo => tr("desc.join_video"),
            Self::JoinAudio => tr("desc.join_audio"),
            Self::Mux => tr("desc.mux"),
            Self::AudioMix => tr("desc.audio_mix"),
            Self::VideoMix => tr("desc.video_mix"),
            Self::Trim => tr("desc.trim"),
            Self::Split => tr("desc.split"),
            Self::Compress => tr("desc.compress"),
            Self::RemoveAudio => tr("desc.remove_audio"),
            Self::Delogo => tr("desc.delogo"),
            Self::ExtractFrames => tr("desc.extract_frames"),
            Self::Rotate => tr("desc.rotate"),
            Self::Speed => tr("desc.speed"),
            Self::Volume => tr("desc.volume"),
            Self::Normalize => tr("desc.normalize"),
            Self::Repair => tr("desc.repair"),
            Self::Download => tr("desc.download"),
            Self::Record => tr("desc.record"),
        }
    }

    pub fn arity(self) -> InputArity {
        match self {
            Self::ImagesToPdf => InputArity::AllAtLeast(1),
            Self::JoinVideo | Self::JoinAudio | Self::AudioMix | Self::PdfMerge => {
                InputArity::AllAtLeast(2)
            }
            Self::Mux | Self::VideoMix => InputArity::Exactly(2),
            Self::Record => InputArity::None,
            _ => InputArity::EachFile,
        }
    }

    pub fn input_kind(self) -> InputKind {
        match self {
            Self::ToImage(_) | Self::ImagesToPdf => InputKind::Images,
            Self::PdfToImages
            | Self::PdfToText
            | Self::PdfToHtml
            | Self::PdfToWord
            | Self::PdfMerge
            | Self::PdfSplit
            | Self::PdfCompress => InputKind::Pdf,
            Self::OfficeToPdf => InputKind::Documents,
            Self::ToEbook(_) => InputKind::Ebooks,
            Self::DvdToVideo => InputKind::Disc,
            Self::CdToAudio => InputKind::Device,
            Self::FolderToIso => InputKind::Folder,
            Self::IsoToCso => InputKind::Iso,
            Self::CsoToIso => InputKind::Cso,
            Self::Download => InputKind::Url,
            Self::Record => InputKind::Capture,
            _ => InputKind::Media,
        }
    }

    /// External programs the tool cannot run without.
    pub fn requires(self) -> &'static [External] {
        match self {
            Self::ToImage(_) | Self::ImagesToPdf | Self::IsoToCso | Self::CsoToIso => &[],
            Self::PdfToImages => &[External::Pdftoppm],
            Self::PdfToText => &[External::Pdftotext],
            Self::PdfToHtml => &[External::Pdftohtml],
            Self::PdfMerge => &[External::Pdfunite],
            Self::PdfSplit => &[External::Pdfseparate],
            Self::PdfCompress => &[External::Ghostscript],
            Self::OfficeToPdf | Self::PdfToWord => &[External::Soffice],
            Self::ToEbook(_) => &[External::EbookConvert],
            Self::FolderToIso => &[External::Xorriso],
            Self::Download => &[External::YtDlp, External::Ffmpeg],
            _ => &[External::Ffmpeg],
        }
    }

    /// The ffmpeg encoder the tool's output depends on, if any.
    pub fn required_encoder(self) -> Option<&'static str> {
        match self {
            Self::ToVideo(f) => Some(f.required_encoder()),
            Self::ToAudio(f) => Some(f.required_encoder()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_has_tools() {
        assert!(Category::ALL.iter().all(|c| !c.tools().is_empty()));
    }

    #[test]
    fn video_from_ext_round_trips_and_aliases() {
        assert_eq!(VideoFormat::from_ext("MKV"), Some(VideoFormat::Mkv));
        assert_eq!(VideoFormat::from_ext("m4v"), Some(VideoFormat::Mp4));
        assert_eq!(VideoFormat::from_ext("3gp"), Some(VideoFormat::ThreeGp));
        assert_eq!(VideoFormat::from_ext("xyz"), None);
    }

    #[test]
    fn native_tools_need_no_externals() {
        for tool in [
            Tool::ToImage(ImageFormat::Png),
            Tool::ImagesToPdf,
            Tool::IsoToCso,
        ] {
            assert!(tool.requires().is_empty(), "{tool:?}");
        }
        assert!(Tool::Trim.requires().contains(&External::Ffmpeg));
    }

    #[test]
    fn every_container_offers_its_default_encoder() {
        for f in VideoFormat::ALL {
            let choices = f.encoder_choices();
            assert_eq!(choices[0], VideoEncoder::Auto, "{f:?}");
        }
    }

    #[test]
    fn tool_labels_are_unique_within_categories() {
        for category in Category::ALL {
            let mut labels: Vec<_> = category.tools().iter().map(|t| t.label()).collect();
            let before = labels.len();
            labels.sort_unstable();
            labels.dedup();
            assert_eq!(before, labels.len(), "{category:?}");
        }
    }
}
