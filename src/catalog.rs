//! The tool catalog: every conversion and utility the app offers.

use serde::{Deserialize, Serialize};

use crate::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Video,
    Audio,
    Image,
    Document,
    Utilities,
}

impl Category {
    pub const ALL: [Self; 5] = [
        Self::Video,
        Self::Audio,
        Self::Image,
        Self::Document,
        Self::Utilities,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Video => tr("category.video"),
            Self::Audio => tr("category.audio"),
            Self::Image => tr("category.image"),
            Self::Document => tr("category.document"),
            Self::Utilities => tr("category.utilities"),
        }
    }

    pub fn tools(self) -> Vec<Tool> {
        match self {
            Self::Video => VideoFormat::ALL.into_iter().map(Tool::ToVideo).collect(),
            Self::Audio => AudioFormat::ALL.into_iter().map(Tool::ToAudio).collect(),
            Self::Image => ImageFormat::ALL.into_iter().map(Tool::ToImage).collect(),
            Self::Document => vec![Tool::ImagesToPdf],
            Self::Utilities => vec![
                Tool::JoinVideo,
                Tool::JoinAudio,
                Tool::Mux,
                Tool::Trim,
                Tool::Compress,
                Tool::RemoveAudio,
                Tool::ExtractFrames,
                Tool::Rotate,
                Tool::Speed,
                Tool::Normalize,
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
    Ts,
    Ogv,
    ThreeGp,
    Gif,
}

impl VideoFormat {
    pub const ALL: [Self; 13] = [
        Self::Mp4,
        Self::Mp4Hevc,
        Self::Mkv,
        Self::Webm,
        Self::Mov,
        Self::Avi,
        Self::Flv,
        Self::Wmv,
        Self::Mpg,
        Self::Ts,
        Self::Ogv,
        Self::ThreeGp,
        Self::Gif,
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
            Self::Ts => "ts",
            Self::Ogv => "ogv",
            Self::ThreeGp => "3gp",
            Self::Gif => "gif",
        }
    }

    /// Format names are technical identifiers and are not translated.
    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4 (H.264)",
            Self::Mp4Hevc => "MP4 (H.265)",
            Self::Mkv => "MKV",
            Self::Webm => "WebM (VP9)",
            Self::Mov => "MOV",
            Self::Avi => "AVI",
            Self::Flv => "FLV",
            Self::Wmv => "WMV",
            Self::Mpg => "MPG (MPEG-2)",
            Self::Ts => "TS",
            Self::Ogv => "OGV (Theora)",
            Self::ThreeGp => "3GP",
            Self::Gif => "GIF",
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioFormat {
    Mp3,
    Aac,
    M4a,
    Flac,
    Wav,
    Ogg,
    Opus,
    Wma,
    Aiff,
    Ac3,
}

impl AudioFormat {
    pub const ALL: [Self; 10] = [
        Self::Mp3,
        Self::Aac,
        Self::M4a,
        Self::Flac,
        Self::Wav,
        Self::Ogg,
        Self::Opus,
        Self::Wma,
        Self::Aiff,
        Self::Ac3,
    ];

    pub fn ext(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Aac => "aac",
            Self::M4a => "m4a",
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Ogg => "ogg",
            Self::Opus => "opus",
            Self::Wma => "wma",
            Self::Aiff => "aiff",
            Self::Ac3 => "ac3",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mp3 => "MP3",
            Self::Aac => "AAC",
            Self::M4a => "M4A",
            Self::Flac => "FLAC",
            Self::Wav => "WAV",
            Self::Ogg => "OGG (Vorbis)",
            Self::Opus => "Opus",
            Self::Wma => "WMA",
            Self::Aiff => "AIFF",
            Self::Ac3 => "AC3",
        }
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
pub enum Tool {
    ToVideo(VideoFormat),
    ToAudio(AudioFormat),
    ToImage(ImageFormat),
    ImagesToPdf,
    JoinVideo,
    JoinAudio,
    Mux,
    Trim,
    Compress,
    RemoveAudio,
    ExtractFrames,
    Rotate,
    Speed,
    Normalize,
}

/// How a tool groups its input files into jobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputArity {
    /// One job per input file.
    EachFile,
    /// One job consuming all files, needing at least this many.
    AllAtLeast(usize),
    /// One job consuming exactly this many files, in order.
    Exactly(usize),
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Self::ToVideo(f) => f.label(),
            Self::ToAudio(f) => f.label(),
            Self::ToImage(f) => f.label(),
            Self::ImagesToPdf => tr("tool.images_to_pdf"),
            Self::JoinVideo => tr("tool.join_video"),
            Self::JoinAudio => tr("tool.join_audio"),
            Self::Mux => tr("tool.mux"),
            Self::Trim => tr("tool.trim"),
            Self::Compress => tr("tool.compress"),
            Self::RemoveAudio => tr("tool.remove_audio"),
            Self::ExtractFrames => tr("tool.extract_frames"),
            Self::Rotate => tr("tool.rotate"),
            Self::Speed => tr("tool.speed"),
            Self::Normalize => tr("tool.normalize"),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::ToVideo(_) => tr("desc.to_video"),
            Self::ToAudio(_) => tr("desc.to_audio"),
            Self::ToImage(_) => tr("desc.to_image"),
            Self::ImagesToPdf => tr("desc.images_to_pdf"),
            Self::JoinVideo => tr("desc.join_video"),
            Self::JoinAudio => tr("desc.join_audio"),
            Self::Mux => tr("desc.mux"),
            Self::Trim => tr("desc.trim"),
            Self::Compress => tr("desc.compress"),
            Self::RemoveAudio => tr("desc.remove_audio"),
            Self::ExtractFrames => tr("desc.extract_frames"),
            Self::Rotate => tr("desc.rotate"),
            Self::Speed => tr("desc.speed"),
            Self::Normalize => tr("desc.normalize"),
        }
    }

    pub fn arity(self) -> InputArity {
        match self {
            Self::ImagesToPdf => InputArity::AllAtLeast(1),
            Self::JoinVideo | Self::JoinAudio => InputArity::AllAtLeast(2),
            Self::Mux => InputArity::Exactly(2),
            _ => InputArity::EachFile,
        }
    }

    /// Image tools run natively; everything else needs ffmpeg.
    pub fn needs_ffmpeg(self) -> bool {
        !matches!(self, Self::ToImage(_) | Self::ImagesToPdf)
    }

    /// Whether the tool reads raster images rather than media streams.
    pub fn takes_images(self) -> bool {
        !self.needs_ffmpeg()
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
    fn image_tools_do_not_need_ffmpeg() {
        assert!(!Tool::ToImage(ImageFormat::Png).needs_ffmpeg());
        assert!(!Tool::ImagesToPdf.needs_ffmpeg());
        assert!(Tool::Trim.needs_ffmpeg());
    }
}
