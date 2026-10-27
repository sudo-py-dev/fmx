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
}

/// Parses `ffprobe -print_format json -show_format -show_streams -show_chapters`.
pub fn parse_ffprobe_json(text: &str) -> Option<MediaInfo> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut info = MediaInfo {
        duration_s: json_duration(value.pointer("/format/duration")),
        ..MediaInfo::default()
    };
    if let Some(streams) = value.get("streams").and_then(|s| s.as_array()) {
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
        if let Some(rest) = line.strip_prefix("Duration:") {
            if info.duration_s.is_none() {
                info.duration_s = rest.split(',').next().and_then(parse_clock);
            }
        } else if line.starts_with("Stream #") {
            parse_stream_line(line, &mut info);
        } else if line.starts_with("Chapter #") {
            info.chapters.extend(parse_chapter(line));
        }
    }
    info
}

fn parse_stream_line(line: &str, info: &mut MediaInfo) {
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
