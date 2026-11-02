//! The tool catalog: every conversion and utility the app offers.

use serde::{Deserialize, Serialize};

use crate::externals::External;
use crate::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Video,
    Audio,
    Utilities,
    Image,
    Document,
    Capture,
    Devices,
    Rom,
}

/// One task group on a category page. Secondary groups stay searchable even
/// while collapsed, so older formats remain available without crowding the page.
pub struct ToolGroup {
    pub key: &'static str,
    pub tools: Vec<Tool>,
    pub secondary: bool,
}

impl ToolGroup {
    pub fn label(&self) -> &'static str {
        tr(self.key)
    }
}

impl Category {
    pub const ALL: [Self; 8] = [
        Self::Video,
        Self::Audio,
        Self::Utilities,
        Self::Image,
        Self::Document,
        Self::Capture,
        Self::Devices,
        Self::Rom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Video => tr("category.video"),
            Self::Audio => tr("category.audio"),
            Self::Utilities => tr("category.utilities"),
            Self::Image => tr("category.image"),
            Self::Document => tr("category.document"),
            Self::Capture => tr("category.capture"),
            Self::Devices => tr("category.devices"),
            Self::Rom => tr("category.rom"),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Video => tr("browse.video"),
            Self::Audio => tr("browse.audio"),
            Self::Utilities => tr("browse.media"),
            Self::Image => tr("browse.image"),
            Self::Document => tr("browse.document"),
            Self::Capture => tr("browse.capture"),
            Self::Devices => tr("browse.devices"),
            Self::Rom => tr("browse.discs"),
        }
    }

    pub fn tools(self) -> Vec<Tool> {
        self.groups()
            .iter()
            .flat_map(|group| group.tools.iter().copied())
            .collect()
    }

    pub fn groups(self) -> &'static [ToolGroup] {
        static GROUPS: std::sync::OnceLock<std::collections::HashMap<Category, Vec<ToolGroup>>> =
            std::sync::OnceLock::new();
        GROUPS
            .get_or_init(|| {
                Self::ALL
                    .into_iter()
                    .map(|category| (category, category.build_groups()))
                    .collect()
            })
            .get(&self)
            .expect("every category has a tool catalog")
            .as_slice()
    }

    fn build_groups(self) -> Vec<ToolGroup> {
        let group = |key, tools| ToolGroup {
            key,
            tools,
            secondary: false,
        };
        let more = |key, tools| ToolGroup {
            key,
            tools,
            secondary: true,
        };
        match self {
            Self::Video => {
                let common = [
                    VideoFormat::Mp4,
                    VideoFormat::Mkv,
                    VideoFormat::Webm,
                    VideoFormat::Mov,
                    VideoFormat::Mp4Hevc,
                    VideoFormat::Avi,
                ];
                vec![
                    group(
                        "tools.video_convert",
                        common.into_iter().map(Tool::ToVideo).collect(),
                    ),
                    group(
                        "tools.video_edit",
                        vec![
                            Tool::JoinVideo,
                            Tool::VideoMix,
                            Tool::Mux,
                            Tool::Crop,
                            Tool::Rotate,
                            Tool::Compress,
                            Tool::RemoveAudio,
                        ],
                    ),
                    group(
                        "tools.video_improve",
                        vec![Tool::Stabilize, Tool::Denoise, Tool::Sharpen, Tool::Delogo],
                    ),
                    group(
                        "tools.subtitles",
                        vec![Tool::AddSubtitles, Tool::ExtractSubtitles],
                    ),
                    group(
                        "tools.frames_animation",
                        vec![
                            Tool::ExtractFrames,
                            Tool::ThumbnailSheet,
                            Tool::ToVideo(VideoFormat::Gif),
                            Tool::ToVideo(VideoFormat::Webp),
                        ],
                    ),
                    more(
                        "tools.video_more",
                        VideoFormat::ALL
                            .into_iter()
                            .filter(|f| !common.contains(f) && !f.is_animation())
                            .map(Tool::ToVideo)
                            .collect(),
                    ),
                ]
            }
            Self::Audio => {
                let common = [
                    AudioFormat::Mp3,
                    AudioFormat::M4a,
                    AudioFormat::Aac,
                    AudioFormat::Flac,
                    AudioFormat::Wav,
                    AudioFormat::Ogg,
                    AudioFormat::Opus,
                    AudioFormat::Alac,
                ];
                vec![
                    group(
                        "tools.audio_convert",
                        common.into_iter().map(Tool::ToAudio).collect(),
                    ),
                    group(
                        "tools.audio_edit",
                        vec![
                            Tool::JoinAudio,
                            Tool::AudioMix,
                            Tool::Volume,
                            Tool::EditTags,
                            Tool::WaveformVideo,
                        ],
                    ),
                    group(
                        "tools.audio_cleanup",
                        vec![
                            Tool::AudioDenoise,
                            Tool::TrimSilence,
                            Tool::Karaoke,
                            Tool::Normalize,
                        ],
                    ),
                    more(
                        "tools.audio_more",
                        AudioFormat::ALL
                            .into_iter()
                            .filter(|f| !common.contains(f))
                            .map(Tool::ToAudio)
                            .collect(),
                    ),
                ]
            }
            Self::Image => {
                let common = [
                    ImageFormat::Jpeg,
                    ImageFormat::Png,
                    ImageFormat::Webp,
                    ImageFormat::Avif,
                ];
                vec![
                    group(
                        "tools.image_convert",
                        common.into_iter().map(Tool::ToImage).collect(),
                    ),
                    group(
                        "tools.image_utilities",
                        vec![Tool::JoinImages, Tool::RemoveImageMetadata],
                    ),
                    more(
                        "tools.image_more",
                        ImageFormat::ALL
                            .into_iter()
                            .filter(|f| !common.contains(f))
                            .map(Tool::ToImage)
                            .collect(),
                    ),
                ]
            }
            Self::Document => vec![
                group(
                    "tools.pdf_create",
                    vec![Tool::OfficeToPdf, Tool::ImagesToPdf],
                ),
                group(
                    "tools.pdf_convert",
                    vec![
                        Tool::PdfToWord,
                        Tool::PdfToExcel,
                        Tool::PdfToText,
                        Tool::PdfToHtml,
                        Tool::PdfToImages,
                    ],
                ),
                group(
                    "tools.pdf_organize",
                    vec![Tool::PdfMerge, Tool::PdfSplit, Tool::PdfCompress],
                ),
                group(
                    "tools.ebooks",
                    EbookFormat::ALL.into_iter().map(Tool::ToEbook).collect(),
                ),
            ],
            Self::Capture => vec![
                group("tools.download", vec![Tool::Download]),
                group(
                    "tools.record_capture",
                    vec![Tool::Record, Tool::RecordAudio, Tool::Screenshot],
                ),
            ],
            Self::Devices => {
                let phones = [
                    Device::Iphone,
                    Device::Ipad,
                    Device::AndroidPhone,
                    Device::AndroidTablet,
                ];
                let sharing = [Device::Youtube, Device::Whatsapp];
                vec![
                    group(
                        "tools.phones",
                        Device::ALL
                            .into_iter()
                            .filter(|d| phones.contains(d))
                            .map(Tool::ToDevice)
                            .collect(),
                    ),
                    group(
                        "tools.tv_games",
                        Device::ALL
                            .into_iter()
                            .filter(|d| !phones.contains(d) && !sharing.contains(d))
                            .map(Tool::ToDevice)
                            .collect(),
                    ),
                    group(
                        "tools.sharing",
                        Device::ALL
                            .into_iter()
                            .filter(|d| sharing.contains(d))
                            .map(Tool::ToDevice)
                            .collect(),
                    ),
                ]
            }
            Self::Rom => vec![
                group(
                    "tools.read_discs",
                    vec![
                        Tool::DvdToVideo,
                        Tool::BluRayToVideo,
                        Tool::CdToAudio,
                        Tool::DiscToIso,
                    ],
                ),
                group(
                    "tools.disc_images",
                    vec![Tool::FolderToIso, Tool::IsoToCso, Tool::CsoToIso],
                ),
            ],
            Self::Utilities => vec![
                group(
                    "tools.media_edit",
                    vec![
                        Tool::Trim,
                        Tool::Split,
                        Tool::Speed,
                        Tool::Reverse,
                        Tool::Fade,
                    ],
                ),
                group("tools.inspect_repair", vec![Tool::MediaInfo, Tool::Repair]),
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
    Ape,
    Alac,
    Caf,
    Au,
}

impl AudioFormat {
    pub const ALL: [Self; 20] = [
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
        Self::Ape,
        Self::Alac,
        Self::Caf,
        Self::Au,
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
            Self::Ape => "ape",
            Self::Alac => "m4a",
            Self::Caf => "caf",
            Self::Au => "au",
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
            Self::Ape => "APE (Monkey's Audio)",
            Self::Alac => "ALAC (Apple Lossless)",
            Self::Caf => "CAF",
            Self::Au => "AU",
        }
    }

    pub fn required_encoder(self) -> &'static str {
        match self {
            Self::Mp3 => "libmp3lame",
            Self::Aac | Self::M4a | Self::M4r | Self::Mka => "aac",
            Self::Flac => "flac",
            Self::Wav | Self::Ape => "pcm_s16le",
            Self::Aiff | Self::Caf | Self::Au => "pcm_s16be",
            Self::Alac => "alac",
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
        matches!(
            self,
            Self::Flac
                | Self::Wav
                | Self::Aiff
                | Self::Wv
                | Self::Ape
                | Self::Alac
                | Self::Caf
                | Self::Au
        )
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
    Avif,
    Jxl,
    Pcx,
    Ppm,
    Pgm,
    Pam,
}

impl ImageFormat {
    pub const ALL: [Self; 15] = [
        Self::Png,
        Self::Jpeg,
        Self::Webp,
        Self::Bmp,
        Self::Tiff,
        Self::Gif,
        Self::Ico,
        Self::Tga,
        Self::Qoi,
        Self::Avif,
        Self::Jxl,
        Self::Pcx,
        Self::Ppm,
        Self::Pgm,
        Self::Pam,
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
            Self::Avif => "avif",
            Self::Jxl => "jxl",
            Self::Pcx => "pcx",
            Self::Ppm => "ppm",
            Self::Pgm => "pgm",
            Self::Pam => "pam",
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
            Self::Avif => "AVIF",
            Self::Jxl => "JPEG XL",
            Self::Pcx => "PCX",
            Self::Ppm => "PPM",
            Self::Pgm => "PGM (grayscale)",
            Self::Pam => "PAM",
        }
    }

    /// The native encoder, or `None` for formats written through ffmpeg.
    pub fn codec(self) -> Option<image::ImageFormat> {
        Some(match self {
            Self::Png => image::ImageFormat::Png,
            Self::Jpeg => image::ImageFormat::Jpeg,
            Self::Webp => image::ImageFormat::WebP,
            Self::Bmp => image::ImageFormat::Bmp,
            Self::Tiff => image::ImageFormat::Tiff,
            Self::Gif => image::ImageFormat::Gif,
            Self::Ico => image::ImageFormat::Ico,
            Self::Tga => image::ImageFormat::Tga,
            Self::Qoi => image::ImageFormat::Qoi,
            Self::Ppm | Self::Pgm | Self::Pam => image::ImageFormat::Pnm,
            Self::Avif | Self::Jxl | Self::Pcx => return None,
        })
    }

    /// ffmpeg encoder flags for formats the native encoders don't cover.
    pub fn ffmpeg_encoder(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Avif => Some(&[
                "-c:v",
                "libaom-av1",
                "-still-picture",
                "1",
                "-crf",
                "30",
                "-cpu-used",
                "6",
            ]),
            Self::Jxl => Some(&["-c:v", "libjxl", "-distance", "1"]),
            Self::Pcx => Some(&["-c:v", "pcx"]),
            _ => None,
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

    /// Written by FMX itself (see `ebook`); Kindle formats need Calibre.
    pub fn is_native(self) -> bool {
        !matches!(self, Self::Mobi | Self::Azw3)
    }

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

/// Ready-made output profiles for phones, tablets, consoles and sites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Device {
    Iphone,
    Ipad,
    AndroidPhone,
    AndroidTablet,
    Psp,
    PsVita,
    Xbox,
    SmartTv,
    Youtube,
    Whatsapp,
}

/// What a device profile constrains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceProfile {
    pub max_width: u32,
    pub max_height: u32,
    /// H.264 profile and level.
    pub profile: &'static str,
    pub level: &'static str,
    pub max_fps: Option<u32>,
    pub max_kbps: u32,
    pub audio_kbps: u32,
}

impl Device {
    pub const ALL: [Self; 10] = [
        Self::Iphone,
        Self::Ipad,
        Self::AndroidPhone,
        Self::AndroidTablet,
        Self::Psp,
        Self::PsVita,
        Self::Xbox,
        Self::SmartTv,
        Self::Youtube,
        Self::Whatsapp,
    ];

    /// Device and site names are trademarks and are not translated.
    pub fn label(self) -> &'static str {
        match self {
            Self::Iphone => "iPhone",
            Self::Ipad => "iPad",
            Self::AndroidPhone => "Android phone",
            Self::AndroidTablet => "Android tablet",
            Self::Psp => "PSP",
            Self::PsVita => "PS Vita",
            Self::Xbox => "Xbox",
            Self::SmartTv => "Smart TV",
            Self::Youtube => "YouTube",
            Self::Whatsapp => "WhatsApp",
        }
    }

    pub fn profile(self) -> DeviceProfile {
        let p =
            |max_width, max_height, profile, level, max_fps, max_kbps, audio_kbps| DeviceProfile {
                max_width,
                max_height,
                profile,
                level,
                max_fps,
                max_kbps,
                audio_kbps,
            };
        match self {
            Self::Iphone => p(1920, 1080, "high", "4.1", None, 8000, 160),
            Self::Ipad => p(2048, 1536, "high", "4.2", None, 10000, 160),
            Self::AndroidPhone => p(1280, 720, "main", "4.0", Some(30), 4000, 128),
            Self::AndroidTablet => p(1920, 1080, "high", "4.1", Some(30), 6000, 160),
            Self::Psp => p(480, 272, "main", "3.0", Some(30), 768, 128),
            Self::PsVita => p(960, 544, "main", "3.1", Some(30), 2500, 128),
            Self::Xbox => p(1920, 1080, "high", "4.1", None, 10000, 192),
            Self::SmartTv => p(1920, 1080, "high", "4.1", None, 12000, 192),
            Self::Youtube => p(3840, 2160, "high", "5.1", None, 40000, 192),
            Self::Whatsapp => p(854, 480, "baseline", "3.1", Some(30), 1200, 96),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tool {
    ToVideo(VideoFormat),
    ToDevice(Device),
    ToAudio(AudioFormat),
    ToImage(ImageFormat),
    JoinImages,
    RemoveImageMetadata,
    MediaInfo,
    ImagesToPdf,
    PdfToImages,
    PdfToText,
    PdfToHtml,
    PdfToWord,
    PdfToExcel,
    OfficeToPdf,
    PdfMerge,
    PdfSplit,
    PdfCompress,
    ToEbook(EbookFormat),
    DvdToVideo,
    BluRayToVideo,
    CdToAudio,
    DiscToIso,
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
    Crop,
    Compress,
    RemoveAudio,
    Delogo,
    AddSubtitles,
    ExtractSubtitles,
    ExtractFrames,
    Rotate,
    Speed,
    Volume,
    Normalize,
    Reverse,
    Fade,
    Stabilize,
    Denoise,
    Sharpen,
    AudioDenoise,
    TrimSilence,
    Karaoke,
    ThumbnailSheet,
    WaveformVideo,
    EditTags,
    Repair,
    Download,
    Record,
    RecordAudio,
    Screenshot,
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
    /// No inputs at all (screen recording and screenshots).
    None,
}

/// What the user supplies as input.
/// Audio file extensions, a subset of [`InputKind::Media`]'s.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "aac", "m4a", "flac", "wav", "ogg", "opus", "wma", "aiff", "aif", "ac3", "amr", "ape",
    "mka", "wv", "mp2", "mmf", "m4r", "caf", "au", "snd",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    Media,
    Images,
    Photos,
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
                "ape", "mka", "wv", "mp2", "mmf", "m4r", "caf", "au", "snd",
            ],
            Self::Images => &[
                "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif", "ico", "tga", "qoi",
                "pnm", "ppm", "pgm", "pbm", "pam", "heic", "heif", "avif", "jxl", "pcx",
            ],
            Self::Photos => &["jpg", "jpeg", "png", "webp", "bmp"],
            Self::Pdf => &["pdf"],
            Self::Documents => &[
                "docx", "odt", "rtf", "xlsx", "xlsm", "xls", "ods", "csv", "tsv", "pptx", "odp",
                "txt", "md", "html", "htm", "doc", "ppt",
            ],
            Self::Ebooks => &[
                "epub", "fb2", "mobi", "azw3", "azw", "pdf", "txt", "docx", "odt", "rtf", "md",
                "html", "htm", "lit", "pdb",
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
            Self::ToDevice(d) => d.label(),
            Self::ToAudio(f) => f.label(),
            Self::ToImage(f) => f.label(),
            Self::ToEbook(f) => f.label(),
            Self::ImagesToPdf => tr("tool.images_to_pdf"),
            Self::JoinImages => tr("tool.join_images"),
            Self::RemoveImageMetadata => tr("tool.remove_image_metadata"),
            Self::MediaInfo => tr("tool.media_info"),
            Self::PdfToExcel => tr("tool.pdf_to_excel"),
            Self::BluRayToVideo => tr("tool.bluray_to_video"),
            Self::DiscToIso => tr("tool.disc_to_iso"),
            Self::Crop => tr("tool.crop"),
            Self::AddSubtitles => tr("tool.add_subtitles"),
            Self::ExtractSubtitles => tr("tool.extract_subtitles"),
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
            Self::Reverse => tr("tool.reverse"),
            Self::Fade => tr("tool.fade"),
            Self::Stabilize => tr("tool.stabilize"),
            Self::Denoise => tr("tool.denoise"),
            Self::Sharpen => tr("tool.sharpen"),
            Self::AudioDenoise => tr("tool.audio_denoise"),
            Self::TrimSilence => tr("tool.trim_silence"),
            Self::Karaoke => tr("tool.karaoke"),
            Self::ThumbnailSheet => tr("tool.thumbnail_sheet"),
            Self::WaveformVideo => tr("tool.waveform_video"),
            Self::EditTags => tr("tool.edit_tags"),

            Self::Repair => tr("tool.repair"),
            Self::Download => tr("tool.download"),
            Self::Record => tr("tool.record"),
            Self::RecordAudio => tr("tool.record_audio"),
            Self::Screenshot => tr("tool.screenshot"),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::ToVideo(_) => tr("desc.to_video"),
            Self::ToDevice(_) => tr("desc.to_device"),
            Self::JoinImages => tr("desc.join_images"),
            Self::RemoveImageMetadata => tr("desc.remove_image_metadata"),
            Self::MediaInfo => tr("desc.media_info"),
            Self::PdfToExcel => tr("desc.pdf_to_excel"),
            Self::BluRayToVideo => tr("desc.bluray_to_video"),
            Self::DiscToIso => tr("desc.disc_to_iso"),
            Self::Crop => tr("desc.crop"),
            Self::AddSubtitles => tr("desc.add_subtitles"),
            Self::ExtractSubtitles => tr("desc.extract_subtitles"),
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
            Self::Reverse => tr("desc.reverse"),
            Self::Fade => tr("desc.fade"),
            Self::Stabilize => tr("desc.stabilize"),
            Self::Denoise => tr("desc.denoise"),
            Self::Sharpen => tr("desc.sharpen"),
            Self::AudioDenoise => tr("desc.audio_denoise"),
            Self::TrimSilence => tr("desc.trim_silence"),
            Self::Karaoke => tr("desc.karaoke"),
            Self::ThumbnailSheet => tr("desc.thumbnail_sheet"),
            Self::WaveformVideo => tr("desc.waveform_video"),
            Self::EditTags => tr("desc.edit_tags"),

            Self::Repair => tr("desc.repair"),
            Self::Download => tr("desc.download"),
            Self::Record => tr("desc.record"),
            Self::RecordAudio => tr("desc.record_audio"),
            Self::Screenshot => tr("desc.screenshot"),
        }
    }

    pub fn arity(self) -> InputArity {
        match self {
            Self::ImagesToPdf => InputArity::AllAtLeast(1),
            Self::JoinImages => InputArity::AllAtLeast(2),
            Self::JoinVideo | Self::JoinAudio | Self::AudioMix | Self::PdfMerge => {
                InputArity::AllAtLeast(2)
            }
            Self::Mux | Self::VideoMix => InputArity::Exactly(2),
            Self::Record | Self::RecordAudio | Self::Screenshot => InputArity::None,
            _ => InputArity::EachFile,
        }
    }

    pub fn input_kind(self) -> InputKind {
        match self {
            Self::ToImage(_) | Self::ImagesToPdf | Self::JoinImages => InputKind::Images,
            Self::PdfToImages
            | Self::PdfToText
            | Self::PdfToHtml
            | Self::PdfToWord
            | Self::PdfToExcel
            | Self::PdfMerge
            | Self::PdfSplit
            | Self::PdfCompress => InputKind::Pdf,
            Self::RemoveImageMetadata => InputKind::Photos,
            Self::OfficeToPdf => InputKind::Documents,
            Self::ToEbook(_) => InputKind::Ebooks,
            Self::DvdToVideo | Self::BluRayToVideo => InputKind::Disc,
            Self::CdToAudio | Self::DiscToIso => InputKind::Device,
            Self::FolderToIso => InputKind::Folder,
            Self::IsoToCso => InputKind::Iso,
            Self::CsoToIso => InputKind::Cso,
            Self::Download => InputKind::Url,
            Self::Record | Self::RecordAudio | Self::Screenshot => InputKind::Capture,
            _ => InputKind::Media,
        }
    }

    /// The sidebar category the tool belongs to.
    pub fn category(self) -> Category {
        Category::ALL
            .into_iter()
            .find(|c| c.groups().iter().any(|group| group.tools.contains(&self)))
            .unwrap_or(Category::Utilities)
    }

    /// Task-group name is searchable even when the group is collapsed.
    pub fn group_label(self) -> &'static str {
        self.category()
            .groups()
            .iter()
            .find(|group| group.tools.contains(&self))
            .map(|group| group.label())
            .unwrap_or("")
    }

    /// External programs the tool cannot run without.
    pub fn requires(self) -> &'static [External] {
        match self {
            Self::ToImage(f) if f.codec().is_none() => &[External::Ffmpeg],
            Self::ToImage(_)
            | Self::ImagesToPdf
            | Self::JoinImages
            | Self::RemoveImageMetadata
            | Self::IsoToCso
            | Self::CsoToIso
            | Self::DiscToIso => &[],
            Self::ToAudio(AudioFormat::Ape) => &[External::Ffmpeg, External::MonkeysAudio],
            Self::PdfToExcel => &[External::Pdftotext],
            Self::PdfToImages => &[External::Pdftoppm],
            Self::PdfToText => &[External::Pdftotext],
            Self::PdfToHtml => &[External::Pdftohtml],
            Self::PdfMerge => &[External::Pdfunite],
            Self::PdfSplit => &[External::Pdfseparate],
            Self::PdfCompress => &[External::Ghostscript],
            // Native readers and writers; LibreOffice is only an optional extra for .doc/.ppt.
            Self::OfficeToPdf => &[],
            Self::PdfToWord => &[External::Pdftotext],
            Self::ToEbook(f) if f.is_native() => &[],
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
            Self::ToDevice(_) => Some("libx264"),
            Self::ToAudio(f) => Some(f.required_encoder()),
            Self::ToImage(ImageFormat::Avif) => Some("libaom-av1"),
            Self::ToImage(ImageFormat::Jxl) => Some("libjxl"),
            Self::ToImage(ImageFormat::Pcx) => Some("pcx"),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_knows_its_category() {
        for c in Category::ALL {
            assert!(c.tools().iter().all(|t| t.category() == c), "{c:?}");
        }
    }

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

#[cfg(test)]
mod organization_tests {
    use super::*;
    use std::collections::HashSet;
    #[test]
    fn task_groups_keep_every_existing_tool_once() {
        let mut tools = HashSet::new();
        for category in Category::ALL {
            let mut keys = HashSet::new();
            for group in category.groups() {
                assert!(!group.tools.is_empty(), "{}", group.key);
                assert!(keys.insert(group.key));
                for &tool in &group.tools {
                    assert!(tools.insert(tool), "duplicate {tool:?}");
                    assert_eq!(tool.category(), category);
                }
            }
        }
        // Inventory before this navigation change: 67 format/device presets
        // and 54 task tools. Reorganization must not silently remove a tool.
        assert_eq!(tools.len(), 121);
        for format in VideoFormat::ALL {
            assert!(tools.contains(&Tool::ToVideo(format)));
        }
        for format in AudioFormat::ALL {
            assert!(tools.contains(&Tool::ToAudio(format)));
        }
        for format in ImageFormat::ALL {
            assert!(tools.contains(&Tool::ToImage(format)));
        }
        for format in EbookFormat::ALL {
            assert!(tools.contains(&Tool::ToEbook(format)));
        }
        for device in Device::ALL {
            assert!(tools.contains(&Tool::ToDevice(device)));
        }
        assert_eq!(Tool::AudioDenoise.category(), Category::Audio);
        assert_eq!(Tool::Crop.category(), Category::Video);
        assert_eq!(Tool::Record.category(), Category::Capture);
    }
}
