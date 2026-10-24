//! Media inspection by parsing the stream summary ffmpeg prints for `-i`.
//! Using ffmpeg itself keeps the app dependent on a single external binary.

/// What we need to know about an input to plan a conversion.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaInfo {
    pub duration_s: Option<f64>,
    pub has_video: bool,
    pub has_audio: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
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
        }
    }
    info
}

fn parse_stream_line(line: &str, info: &mut MediaInfo) {
    if line.contains(": Audio:") {
        info.has_audio = true;
    } else if line.contains(": Video:") && !line.contains("(attached pic)") && !info.has_video {
        // Only the first real video stream decides the frame size.
        info.has_video = true;
        if let Some((w, h)) = line.split([',', ' ']).find_map(parse_frame_size) {
            info.width = Some(w);
            info.height = Some(h);
        }
    }
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
    fn unknown_duration_is_none() {
        let info = parse_ffmpeg_info("  Duration: N/A, bitrate: N/A");
        assert_eq!(info.duration_s, None);
    }

    #[test]
    fn hex_codec_tags_are_not_frame_sizes() {
        assert_eq!(parse_frame_size("0x31637661"), None);
        assert_eq!(parse_frame_size("640x360"), Some((640, 360)));
    }
}
