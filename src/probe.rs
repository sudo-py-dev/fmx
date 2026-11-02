//! Media inspection from ffprobe JSON, with a fallback parser for ffmpeg's `-i`
//! banner so probing still works when only ffmpeg is present.

/// What we need to know about an input to plan a conversion.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaInfo {
    pub duration_s: Option<f64>,
    pub has_video: bool,
    pub has_audio: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// `(start, end)` seconds of each chapter; audio-CD tracks appear as chapters.
    pub chapters: Vec<(f64, f64)>,
    pub has_subtitles: bool,
    /// Frame rate of the first video stream.
    pub fps: Option<f64>,
    /// None means this probe could not read tags (for example the banner fallback).
    pub tags: Option<AudioTags>,
    pub format: Option<String>,
    pub bitrate_bps: Option<u64>,
    pub streams: Vec<StreamInfo>,
    pub stream_count: usize,
}

/// Structured stream details for the read-only inspector, bounded before rendering.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamInfo {
    pub index: u32,
    pub kind: String,
    pub codec: Option<String>,
    pub bitrate_bps: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub language: Option<String>,
    pub attached_picture: bool,
}

const MAX_INFO_STREAMS: usize = 128;

fn detail_text(value: Option<&serde_json::Value>) -> Option<String> {
    value?
        .as_str()
        .map(|s| display_tag(s).chars().take(128).collect())
}

fn positive_u64(value: Option<&serde_json::Value>) -> Option<u64> {
    let value = value?;
    let number = value.as_u64().or_else(|| value.as_str()?.parse().ok())?;
    (number > 0).then_some(number)
}

/// Display values from untrusted metadata. Never interpret these as paths or markup.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub has_cover: bool,
    pub primary_scopes: [crate::tagging::TagScope; 3],
    pub extra: Vec<crate::tagging::TagField>,
}

pub const MAX_TAG_CHARS: usize = 1024;

pub fn display_tag(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c,
        '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')
        })
        .take(MAX_TAG_CHARS)
        .collect()
}

fn find_tag<'a>(tags: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    tags.as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))?
        .1
        .as_str()
}

fn audio_tags(value: &serde_json::Value) -> AudioTags {
    use crate::tagging::{MAX_EXTRA_TAGS, TagField, TagScope};
    let audio = value["streams"]
        .as_array()
        .and_then(|streams| streams.iter().find(|s| s["codec_type"] == "audio"));
    let mut primary_scopes = [TagScope::File; 3];
    let mut values = Vec::new();
    for (index, key) in ["title", "artist", "album"].into_iter().enumerate() {
        let file = value
            .pointer("/format/tags")
            .and_then(|tags| find_tag(tags, key));
        let stream = audio.and_then(|s| find_tag(&s["tags"], key));
        if file.is_none() && stream.is_some() {
            primary_scopes[index] = TagScope::Audio;
        }
        values.push(file.or(stream).map(display_tag).unwrap_or_default());
    }
    let mut extra = Vec::new();
    let mut add = |tags: &serde_json::Value, scope: TagScope| {
        if let Some(tags) = tags.as_object() {
            for (key, value) in tags {
                if extra.len() >= MAX_EXTRA_TAGS {
                    break;
                }
                let primary =
                    ["title", "artist", "album"]
                        .iter()
                        .enumerate()
                        .any(|(index, name)| {
                            key.eq_ignore_ascii_case(name) && primary_scopes[index] == scope
                        });
                if primary || !crate::tagging::valid_key(key) {
                    continue;
                }
                if let Some(value) = value.as_str() {
                    extra.push(TagField {
                        scope,
                        key: key.clone(),
                        value: crate::tagging::display_value(value),
                        write: false,
                    });
                }
            }
        }
    };
    add(&value["format"]["tags"], TagScope::File);
    if let Some(audio) = audio {
        add(&audio["tags"], TagScope::Audio);
    }
    let mut covers = 0;
    if let Some(streams) = value["streams"].as_array() {
        for stream in streams.iter().filter(|s| {
            s["codec_type"] == "video" && json_flag(s.pointer("/disposition/attached_pic"))
        }) {
            add(&stream["tags"], TagScope::Cover(covers));
            covers += 1;
            if covers >= 128 {
                break;
            }
        }
    }
    AudioTags {
        title: values.remove(0),
        artist: values.remove(0),
        album: values.remove(0),
        has_cover: covers > 0,
        primary_scopes,
        extra,
    }
}

/// Parses `ffprobe -print_format json -show_format -show_streams -show_chapters`.
pub fn parse_ffprobe_json(text: &str) -> Option<MediaInfo> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value.as_object()?;
    let mut info = MediaInfo {
        duration_s: json_duration(value.pointer("/format/duration")),
        tags: Some(audio_tags(&value)),
        format: detail_text(value.pointer("/format/format_name")),
        bitrate_bps: positive_u64(value.pointer("/format/bit_rate")),
        ..MediaInfo::default()
    };
    if let Some(streams) = value.get("streams").and_then(|s| s.as_array()) {
        info.stream_count = streams.len();
        for (index, stream) in streams.iter().take(MAX_INFO_STREAMS).enumerate() {
            info.streams.push(StreamInfo {
                index: stream
                    .get("index")
                    .and_then(|v| v.as_u64())
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(index as u32),
                kind: detail_text(stream.get("codec_type")).unwrap_or_else(|| "unknown".into()),
                codec: detail_text(stream.get("codec_name")),
                bitrate_bps: positive_u64(stream.get("bit_rate")),
                width: json_u32(stream.get("width")),
                height: json_u32(stream.get("height")),
                sample_rate: positive_u64(stream.get("sample_rate"))
                    .and_then(|n| u32::try_from(n).ok()),
                channels: json_u32(stream.get("channels")),
                language: detail_text(stream.pointer("/tags/language")),
                attached_picture: json_flag(stream.pointer("/disposition/attached_pic")),
            });
        }
        for stream in streams {
            match stream.get("codec_type").and_then(|t| t.as_str()) {
                Some("audio") => info.has_audio = true,
                Some("subtitle") => info.has_subtitles = true,
                // The first real video stream decides size and rate; cover art is skipped.
                Some("video")
                    if !info.has_video
                        && !json_flag(stream.pointer("/disposition/attached_pic")) =>
                {
                    info.has_video = true;
                    info.width = json_u32(stream.get("width"));
                    info.height = json_u32(stream.get("height"));
                    info.fps = json_rate(stream.get("avg_frame_rate"))
                        .or_else(|| json_rate(stream.get("r_frame_rate")));
                }
                _ => {}
            }
        }
    }
    if let Some(chapters) = value.get("chapters").and_then(|c| c.as_array()) {
        for chapter in chapters {
            let start = json_duration(chapter.get("start_time"));
            let end = json_duration(chapter.get("end_time"));
            if let (Some(start), Some(end)) = (start, end)
                && end > start
            {
                info.chapters.push((start, end));
            }
        }
    }
    Some(info)
}

fn json_duration(value: Option<&serde_json::Value>) -> Option<f64> {
    let secs = match value? {
        serde_json::Value::String(s) => s.parse().ok()?,
        serde_json::Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    (secs.is_finite() && secs >= 0.0).then_some(secs)
}

fn json_u32(value: Option<&serde_json::Value>) -> Option<u32> {
    let n = u32::try_from(value?.as_u64()?).ok()?;
    (n > 0).then_some(n)
}

fn json_flag(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0) != 0,
        Some(serde_json::Value::Bool(b)) => *b,
        _ => false,
    }
}

/// Parses ffprobe's `num/den` frame-rate strings.
fn json_rate(value: Option<&serde_json::Value>) -> Option<f64> {
    let text = value?.as_str()?;
    let (num, den) = text.split_once('/')?;
    let num: f64 = num.parse().ok()?;
    let den: f64 = den.parse().ok()?;
    (den > 0.0 && num.is_finite()).then_some(num / den)
}

/// Parses ffmpeg's banner output (stderr of `ffmpeg -i <file>`).
pub fn parse_ffmpeg_info(stderr: &str) -> MediaInfo {
    let mut info = MediaInfo::default();
    for line in stderr.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("Input #0, ") {
            info.format = rest
                .split_once(", from ")
                .map(|(name, _)| display_tag(name));
        } else if let Some(rest) = line.strip_prefix("Duration:") {
            if info.duration_s.is_none() {
                info.duration_s = rest.split(',').next().and_then(parse_clock);
            }
            info.bitrate_bps = bitrate_from_banner(rest);
        } else if line.starts_with("Stream #") {
            parse_stream_line(line, &mut info);
        } else if line.starts_with("Chapter #") {
            info.chapters.extend(parse_chapter(line));
        }
    }
    info
}

fn bitrate_from_banner(line: &str) -> Option<u64> {
    let part = line.split(',').find(|part| part.contains("kb/s"))?;
    let value = part
        .trim()
        .trim_start_matches("bitrate:")
        .split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()?;
    (value.is_finite() && value > 0.0 && value < u64::MAX as f64 / 1000.0)
        .then_some((value * 1000.0) as u64)
}

fn parse_stream_line(line: &str, info: &mut MediaInfo) {
    info.stream_count += 1;
    if info.streams.len() < MAX_INFO_STREAMS {
        let (kind, rest) = [
            ("audio", ": Audio:"),
            ("video", ": Video:"),
            ("subtitle", ": Subtitle:"),
            ("data", ": Data:"),
            ("attachment", ": Attachment:"),
        ]
        .into_iter()
        .find_map(|(kind, marker)| line.split_once(marker).map(|(_, rest)| (kind, rest)))
        .unwrap_or(("unknown", ""));
        let index = line
            .split('#')
            .nth(1)
            .and_then(|s| s.split(':').nth(1))
            .map(|s| {
                s.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .and_then(|s| s.parse().ok())
            .unwrap_or(info.streams.len() as u32);
        let codec = rest
            .split_whitespace()
            .next()
            .map(|s| display_tag(s.trim_end_matches(',')));
        let sample_rate = rest
            .split(',')
            .find_map(|part| part.trim().strip_suffix(" Hz").and_then(|n| n.parse().ok()));
        let channels = if rest.split(',').any(|s| s.trim() == "stereo") {
            Some(2)
        } else if rest.split(',').any(|s| s.trim() == "mono") {
            Some(1)
        } else {
            None
        };
        let dimensions = rest.split([',', ' ']).find_map(parse_frame_size);
        info.streams.push(StreamInfo {
            index,
            kind: kind.into(),
            codec,
            sample_rate,
            channels,
            bitrate_bps: bitrate_from_banner(rest),
            width: dimensions.map(|s| s.0),
            height: dimensions.map(|s| s.1),
            attached_picture: line.contains("(attached pic)"),
            ..Default::default()
        });
    }

    if line.contains(": Audio:") {
        info.has_audio = true;
    } else if line.contains(": Subtitle:") {
        info.has_subtitles = true;
    } else if line.contains(": Video:") && !line.contains("(attached pic)") && !info.has_video {
        // Only the first real video stream decides the frame size.
        info.has_video = true;
        if let Some((w, h)) = line.split([',', ' ']).find_map(parse_frame_size) {
            info.width = Some(w);
            info.height = Some(h);
        }
        info.fps = line
            .split(',')
            .find_map(|part| part.trim().strip_suffix(" fps"))
            .and_then(|n| n.trim().parse().ok());
    }
}

/// Parses `Chapter #0:1: start 245.200000, end 480.000000`.
fn parse_chapter(line: &str) -> Option<(f64, f64)> {
    let (_, times) = line.split_once("start ")?;
    let (start, end) = times.split_once(", end ")?;
    let start: f64 = start.trim().parse().ok()?;
    let end: f64 = end.trim().parse().ok()?;
    (end > start).then_some((start, end))
}

fn parse_frame_size(token: &str) -> Option<(u32, u32)> {
    let (w, h) = token.trim().split_once('x')?;
    let w = w.parse().ok().filter(|&w| w > 0)?;
    let h = h.parse().ok().filter(|&h| h > 0)?;
    Some((w, h))
}

/// Parses `hh:mm:ss.ff`; returns `None` for `N/A` or garbage.
fn parse_clock(text: &str) -> Option<f64> {
    let mut parts = text.trim().split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    let total = h * 3600.0 + m * 60.0 + s;
    (parts.next().is_none() && total.is_finite()).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'in.mp4':
  Duration: 00:01:02.50, start: 0.000000, bitrate: 1205 kb/s
  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 1920x1080 [SAR 1:1 DAR 16:9], 1070 kb/s, 30 fps
  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp, 128 kb/s
At least one output file must be specified";

    #[test]
    fn parses_typical_mp4() {
        let info = parse_ffmpeg_info(SAMPLE);
        assert_eq!(info.duration_s, Some(62.5));
        assert!(info.has_video && info.has_audio);
        assert_eq!((info.width, info.height), (Some(1920), Some(1080)));
        assert_eq!(info.fps, Some(30.0));
        assert!(!info.has_subtitles);
    }

    #[test]
    fn subtitle_streams_are_detected() {
        let info = parse_ffmpeg_info("  Stream #0:2(eng): Subtitle: subrip (srt)");
        assert!(info.has_subtitles);
    }

    #[test]
    fn cover_art_is_not_video() {
        let text = "  Duration: 00:03:00.00, start: 0\n  \
            Stream #0:0: Audio: mp3, 44100 Hz, stereo\n  \
            Stream #0:1: Video: mjpeg (Baseline), yuvj420p, 500x500, 90k tbr (attached pic)";
        let info = parse_ffmpeg_info(text);
        assert!(info.has_audio && !info.has_video);
        assert_eq!(info.width, None);
    }

    #[test]
    fn chapters_become_tracks() {
        let text = "    Chapter #0:0: start 0.000000, end 245.200000\n    Chapter #0:1: start 245.200000, end 480.000000";
        assert_eq!(
            parse_ffmpeg_info(text).chapters,
            vec![(0.0, 245.2), (245.2, 480.0)]
        );
    }

    #[test]
    fn unknown_duration_is_none() {
        let info = parse_ffmpeg_info("  Duration: N/A, bitrate: N/A");
        assert_eq!(info.duration_s, None);
    }

    #[test]
    fn hex_codec_tags_are_not_frame_sizes() {
        assert_eq!(parse_frame_size("0x31637661"), None);
        assert_eq!(parse_frame_size("640x360"), Some((640, 360)));
    }

    const FFPROBE_SAMPLE: &str = r#"{
      "streams": [
        {"codec_type": "video", "width": 1920, "height": 1080, "avg_frame_rate": "30/1",
         "disposition": {"attached_pic": 0}},
        {"codec_type": "audio"},
        {"codec_type": "subtitle"}
      ],
      "format": {"duration": "62.500000"},
      "chapters": [
        {"start_time": "0.000000", "end_time": "245.200000"},
        {"start_time": "245.200000", "end_time": "480.000000"}
      ]
    }"#;

    #[test]
    fn parses_ffprobe_json() {
        let info = parse_ffprobe_json(FFPROBE_SAMPLE).expect("json");
        assert_eq!(info.duration_s, Some(62.5));
        assert!(info.has_video && info.has_audio && info.has_subtitles);
        assert_eq!((info.width, info.height), (Some(1920), Some(1080)));
        assert_eq!(info.fps, Some(30.0));
        assert_eq!(info.chapters, vec![(0.0, 245.2), (245.2, 480.0)]);
    }

    #[test]
    fn ffprobe_cover_art_is_not_video() {
        let text = r#"{"streams":[
            {"codec_type":"audio"},
            {"codec_type":"video","width":500,"height":500,"disposition":{"attached_pic":1}}
        ],"format":{"duration":"180.0"}}"#;
        let info = parse_ffprobe_json(text).expect("json");
        assert!(info.has_audio && !info.has_video);
        assert_eq!(info.width, None);
    }

    #[test]
    fn rejects_garbage_ffprobe_json() {
        assert!(parse_ffprobe_json("not json").is_none());
    }
}

#[cfg(test)]
mod tag_tests {
    use super::*;
    #[test]
    fn tags_use_case_insensitive_names_stream_fallback_and_actual_artwork() {
        let info = parse_ffprobe_json(r#"{"format":{"tags":{"TITLE":"שלום café"}},"streams":[{"codec_type":"audio","tags":{"ARTIST":"Artist","album":"Album"}},{"codec_type":"video","disposition":{"attached_pic":1}}]}"#).unwrap();
        assert_eq!(
            info.tags,
            Some(AudioTags {
                title: "שלום café".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                has_cover: true,
                primary_scopes: [
                    crate::tagging::TagScope::File,
                    crate::tagging::TagScope::Audio,
                    crate::tagging::TagScope::Audio
                ],
                extra: Vec::new(),
            })
        );
        assert!(parse_ffmpeg_info("").tags.is_none());
    }
    #[test]
    fn untrusted_display_values_are_bounded_and_cannot_inject_direction_controls() {
        let value = format!("Title\n\0\u{202e}{}", "x".repeat(10_000));
        let info =
            parse_ffprobe_json(&serde_json::json!({"format":{"tags":{"title":value}}}).to_string())
                .unwrap();
        let title = info.tags.unwrap().title;
        assert_eq!(title.chars().count(), MAX_TAG_CHARS);
        assert!(title.starts_with("Title"));
        assert!(!title.chars().any(|c| c.is_control() || c == '\u{202e}'));
    }
}

#[cfg(test)]
mod inspector_tests {
    use super::*;
    #[test]
    fn structured_inspector_reads_codecs_bitrates_audio_and_language() {
        let info = parse_ffprobe_json(r#"{"format":{"format_name":"mov,mp4","bit_rate":"1234567"},"streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"bit_rate":"1000000"},{"index":3,"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2,"bit_rate":"192000","tags":{"language":"heb"}}]}"#).unwrap();
        assert_eq!(info.format.as_deref(), Some("mov,mp4"));
        assert_eq!(info.bitrate_bps, Some(1_234_567));
        assert_eq!(info.stream_count, 2);
        assert_eq!(info.streams[0].codec.as_deref(), Some("h264"));
        assert_eq!(info.streams[1].index, 3);
        assert_eq!(info.streams[1].sample_rate, Some(48_000));
        assert_eq!(info.streams[1].channels, Some(2));
        assert_eq!(info.streams[1].language.as_deref(), Some("heb"));
        assert_eq!(info.streams[1].bitrate_bps, Some(192_000));
    }
    #[test]
    fn inspector_bounds_streams_and_untrusted_strings_and_rejects_bad_numbers() {
        let text = serde_json::json!({"format":{"bit_rate":"-1"},"streams":vec![serde_json::json!({"codec_type":"audio","codec_name":format!("aac\n\u{202e}{}", "x".repeat(1000)),"sample_rate":"NaN"}); 200]}).to_string();
        let info = parse_ffprobe_json(&text).unwrap();
        assert_eq!(info.stream_count, 200);
        assert_eq!(info.streams.len(), MAX_INFO_STREAMS);
        assert_eq!(info.bitrate_bps, None);
        assert_eq!(info.streams[0].sample_rate, None);
        let codec = info.streams[0].codec.as_ref().unwrap();
        assert_eq!(codec.chars().count(), 128);
        assert!(!codec.contains('\n') && !codec.contains('\u{202e}'));
    }
    #[test]
    fn banner_fallback_reads_codec_bitrate_and_audio_details() {
        let info = parse_ffmpeg_info(
            "Input #0, matroska,webm, from 'clip.mkv':\nDuration: 00:00:03.00, bitrate: 1205 kb/s\nStream #0:0: Video: h264, yuv420p, 640x360, 30 fps\nStream #0:1: Audio: aac, 48000 Hz, stereo, 192 kb/s",
        );
        assert_eq!(info.format.as_deref(), Some("matroska,webm"));
        assert_eq!(info.bitrate_bps, Some(1_205_000));
        assert_eq!(info.streams[0].codec.as_deref(), Some("h264"));
        assert_eq!(info.streams[1].sample_rate, Some(48_000));
        assert_eq!(info.streams[1].channels, Some(2));
    }
}

#[cfg(test)]
mod expanded_tag_tests {
    use super::*;
    use crate::tagging::TagScope;
    #[test]
    fn file_audio_and_cover_metadata_are_loaded_separately() {
        let info = parse_ffprobe_json(r#"{"format":{"tags":{"purl":"https://www.youtube.com/watch?v=zIbT9_TKZKo","comment":"source URL","date":"2026","artist":"EyalGolanOfficial"}},"streams":[{"codec_type":"audio","tags":{"encoder":"Lavc62.28"}},{"codec_type":"video","disposition":{"attached_pic":1},"tags":{"title":"Album cover","comment":"Cover (front)"}}]}"#).unwrap();
        let tags = info.tags.unwrap();
        assert!(tags.title.is_empty());
        assert_eq!(tags.artist, "EyalGolanOfficial");
        for (scope, key, value) in [
            (TagScope::File, "date", "2026"),
            (TagScope::File, "comment", "source URL"),
            (TagScope::Audio, "encoder", "Lavc62.28"),
            (TagScope::Cover(0), "title", "Album cover"),
            (TagScope::Cover(0), "comment", "Cover (front)"),
        ] {
            assert!(
                tags.extra
                    .iter()
                    .any(|field| field.scope == scope && field.key == key && field.value == value)
            );
        }
    }
}
