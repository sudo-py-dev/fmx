//! The video downloader: yt-dlp's command line, its progress lines, and the
//! details shown for a link before downloading.

use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::i18n::{tr, trf};
use crate::options::{DownloadAudio, DownloadContainer, DownloadKind, DownloadOptions};
use crate::process::{KillOnDrop, command};

/// Marks the progress lines FMX asks yt-dlp to print.
const PROGRESS_TAG: &str = "FMX|";
/// Bytes, total, estimated total, speed, seconds left, playlist item, item count.
const PROGRESS_TEMPLATE: &str = "download:FMX|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s|%(info.playlist_index)s|%(info.n_entries)s";
/// Link details larger than this are not read (a flat playlist is a few MB at most).
const MAX_INFO_BYTES: u64 = 32 << 20;
const INFO_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_THUMBNAIL_BYTES: u64 = 8 << 20;

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// The yt-dlp arguments that download `url` into `out_dir` as `opts` asks,
/// using our ffmpeg for merging and conversion.
pub fn args(url: &str, out_dir: &Path, ffmpeg: &Path, opts: &DownloadOptions) -> Vec<OsString> {
    let name = if opts.playlist {
        "%(playlist_title,title).100B/%(playlist_index)03d - %(title).150B [%(id)s].%(ext)s"
    } else {
        "%(title).150B [%(id)s].%(ext)s"
    };
    let mut template = out_dir.as_os_str().to_owned();
    template.push(std::path::MAIN_SEPARATOR_STR);
    template.push(name);

    let mut args = os(&[
        "--newline",
        "--no-mtime",
        // Titles keep their own letters but lose characters Windows forbids.
        "--windows-filenames",
        "--progress-template",
        PROGRESS_TEMPLATE,
        if opts.playlist {
            "--yes-playlist"
        } else {
            "--no-playlist"
        },
    ]);
    args.extend([
        "--ffmpeg-location".into(),
        ffmpeg.into(),
        "-o".into(),
        template,
    ]);
    let video = opts.kind == DownloadKind::Video;
    if video {
        // Sorting (not filtering) keeps the sharpest picture up to the limit and,
        // among equals, prefers codecs the container plays best: H.264/AAC in MP4
        // works on every phone and TV, VP9/Opus suits WebM.
        let mut sort = opts
            .max_height
            .map(|h| format!("res:{h}"))
            .into_iter()
            .collect::<Vec<_>>();
        match opts.container {
            DownloadContainer::Mp4 => sort.extend(["vcodec:h264".into(), "acodec:aac".into()]),
            DownloadContainer::Webm => sort.extend(["vcodec:vp9".into(), "acodec:opus".into()]),
            DownloadContainer::Mkv => {}
        }
        args.extend(os(&["-f", "bv*+ba/b"]));
        if !sort.is_empty() {
            args.extend(os(&["-S", &sort.join(",")]));
        }
        args.extend(os(&["--merge-output-format", opts.container.ext()]));
        if opts.subtitles {
            let langs = clean_langs(&opts.subtitle_langs);
            // --embed-subs alone fetches the subtitles and deletes the files once embedded.
            args.extend(os(&["--sub-langs", &langs, "--embed-subs"]));
        }
    } else {
        args.extend(os(&[
            "-f",
            "ba/b",
            "-x",
            "--audio-format",
            opts.audio_format.ext(),
            "--audio-quality",
            "0",
        ]));
    }
    if opts.embed_thumbnail && supports_cover(opts) {
        args.extend(os(&["--embed-thumbnail"]));
    }
    if opts.embed_metadata {
        args.extend(os(&["--embed-metadata"]));
        if video {
            args.extend(os(&["--embed-chapters"]));
        }
    }
    if let Some(kbps) = opts.rate_limit_kbps {
        let kbps = kbps.clamp(1, crate::options::MAX_RATE_LIMIT_KBPS);
        args.extend(os(&["--limit-rate", &format!("{kbps}K")]));
    }
    // Options end here, so a link can never be read as one.
    args.extend(["--".into(), url.trim().into()]);
    args
}

/// Whether the chosen format can hold cover art.
pub fn supports_cover(opts: &DownloadOptions) -> bool {
    match opts.kind {
        DownloadKind::Video => opts.container != DownloadContainer::Webm,
        DownloadKind::Audio => opts.audio_format != DownloadAudio::Wav,
    }
}

/// Keeps only language codes, commas, dashes and `*` (yt-dlp's wildcard).
fn clean_langs(text: &str) -> String {
    let langs: String = text
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ',' | '-' | '*' | '.'))
        .take(crate::options::MAX_SUBTITLE_LANGS_LEN)
        .collect();
    if langs.is_empty() {
        "en".to_owned()
    } else {
        langs
    }
}

/// One progress report from yt-dlp, already turned into words.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// Overall share done (whole playlist), when known.
    pub fraction: Option<f32>,
    pub detail: String,
}

fn number(field: &str) -> Option<f64> {
    field
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.0)
}

/// `12.4 MB`, using decimal units like file managers.
pub fn human_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `1:05` or `1:02:05`.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Reads one line of yt-dlp output: a progress line from our template, or a
/// line announcing a processing step.
pub fn parse_progress(line: &str) -> Option<Progress> {
    if let Some(rest) = line.trim().strip_prefix(PROGRESS_TAG) {
        let f: Vec<&str> = rest.split('|').collect();
        let field = |i: usize| f.get(i).and_then(|v| number(v));
        let done = field(0)?;
        let total = field(1).or(field(2)).filter(|t| *t > 0.0);
        let item = match (field(5), field(6)) {
            (Some(i), Some(n)) if n >= 1.0 && i >= 1.0 && i <= n => Some((i, n)),
            _ => None,
        };
        let file_fraction = total.map(|t| (done / t).clamp(0.0, 1.0));
        let fraction = match (file_fraction, item) {
            (Some(p), Some((i, n))) => Some((i - 1.0 + p) / n),
            (Some(p), None) => Some(p),
            (None, _) => None,
        }
        .map(|p| p as f32);
        let mut parts = vec![match total {
            Some(t) => trf(
                "download.progress_of",
                &[("done", &human_bytes(done)), ("total", &human_bytes(t))],
            ),
            None => human_bytes(done),
        }];
        if let Some(speed) = field(3).filter(|s| *s > 0.0) {
            parts.push(format!("{}/s", human_bytes(speed)));
        }
        if let Some(eta) = field(4) {
            parts.push(trf("download.left", &[("time", &clock(eta))]));
        }
        if let Some((i, n)) = item {
            parts.push(trf("download.item", &[("index", &i), ("count", &n)]));
        }
        return Some(Progress {
            fraction,
            detail: parts.join(" · "),
        });
    }
    let step = match line.split(']').next()?.trim_start_matches('[') {
        "Merger" => "download.step_merge",
        "ExtractAudio" => "download.step_audio",
        "EmbedSubtitle" => "download.step_subtitles",
        "EmbedThumbnail" => "download.step_cover",
        "Metadata" => "download.step_metadata",
        "VideoRemuxer" | "VideoConvertor" => "download.step_convert",
        _ => return None,
    };
    Some(Progress {
        fraction: None,
        detail: tr(step).to_owned(),
    })
}

/// What a link points to, shown before downloading.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkInfo {
    pub title: String,
    pub channel: Option<String>,
    pub site: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    /// The tallest video picture offered.
    pub max_height: Option<u32>,
    /// Items, when the link is a playlist.
    pub playlist_count: Option<u64>,
}

#[derive(Deserialize)]
struct RawInfo {
    #[serde(rename = "_type")]
    kind: Option<String>,
    title: Option<String>,
    uploader: Option<String>,
    channel: Option<String>,
    extractor_key: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    #[serde(default)]
    thumbnails: Vec<RawThumbnail>,
    #[serde(default)]
    formats: Vec<RawFormat>,
    playlist_count: Option<u64>,
    entries: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
struct RawThumbnail {
    url: String,
}

#[derive(Deserialize)]
struct RawFormat {
    height: Option<u32>,
    vcodec: Option<String>,
}

/// Turns yt-dlp's `-J` answer into the details Fmx shows.
pub fn parse_info(json: &str) -> Option<LinkInfo> {
    let raw: RawInfo = serde_json::from_str(json).ok()?;
    let playlist = raw.kind.as_deref() == Some("playlist");
    let max_height = raw
        .formats
        .iter()
        .filter(|f| f.vcodec.as_deref().is_some_and(|v| v != "none"))
        .filter_map(|f| f.height)
        .max();
    // Only web thumbnails are fetched later.
    let thumbnail = raw
        .thumbnail
        .or_else(|| raw.thumbnails.last().map(|t| t.url.clone()))
        .filter(|u| u.starts_with("https://"));
    Some(LinkInfo {
        title: raw.title.unwrap_or_default(),
        channel: raw.channel.or(raw.uploader),
        site: raw.extractor_key,
        duration: raw.duration,
        thumbnail,
        max_height,
        playlist_count: playlist
            .then(|| raw.playlist_count.or(raw.entries.map(|e| e.len() as u64)))
            .flatten(),
    })
}

/// Asks yt-dlp what `url` is, without downloading it. Blocks; run it off the UI thread.
pub fn fetch_info(ytdlp: &Path, url: &str, playlist: bool) -> Result<LinkInfo, String> {
    let mut child = command(ytdlp)
        .args([
            "-J",
            "--flat-playlist",
            "--no-warnings",
            if playlist {
                "--yes-playlist"
            } else {
                "--no-playlist"
            },
            "--",
            url.trim(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(KillOnDrop)
        .map_err(|e| e.to_string())?;
    let stdout = child.0.stdout.take().ok_or("no output")?;
    let stderr = child.0.stderr.take();
    // Reading in a thread lets the timeout below stop a stuck yt-dlp.
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        stdout
            .take(MAX_INFO_BYTES)
            .read_to_string(&mut text)
            .map(|_| text)
    });
    let started = Instant::now();
    while !reader.is_finished() {
        if started.elapsed() > INFO_TIMEOUT {
            return Err(tr("download.info_timeout").to_owned());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let text = reader
        .join()
        .map_err(|_| "reader failed".to_owned())?
        .map_err(|e| e.to_string())?;
    let status = child.0.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        let mut message = String::new();
        if let Some(err) = stderr {
            let _ = err.take(64 << 10).read_to_string(&mut message);
        }
        let line = message
            .lines()
            .rev()
            .find(|l| l.starts_with("ERROR"))
            .unwrap_or(message.lines().last().unwrap_or_default())
            .trim_start_matches("ERROR: ")
            .to_owned();
        return Err(if line.is_empty() {
            tr("download.info_failed").to_owned()
        } else {
            line
        });
    }
    parse_info(&text).ok_or_else(|| tr("download.info_failed").to_owned())
}

/// Downloads and decodes a thumbnail. Blocks; run it off the UI thread.
pub fn fetch_thumbnail(url: &str) -> Result<image::RgbaImage, String> {
    if !url.starts_with("https://") {
        return Err("not https".to_owned());
    }
    let bytes = crate::net::agent()
        .get(url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(MAX_THUMBNAIL_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    let image = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    // Small is enough for the card and keeps the texture light.
    Ok(image.thumbnail(480, 270).into_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(args: &[OsString]) -> String {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn build(opts: &DownloadOptions) -> String {
        text(&args(
            "https://e.com/v",
            Path::new("/o"),
            Path::new("/usr/bin/ffmpeg"),
            opts,
        ))
    }

    #[test]
    fn options_end_before_the_link() {
        let t = build(&DownloadOptions::default());
        assert!(t.ends_with("-- https://e.com/v"));
        assert!(t.contains("--no-playlist"));
        assert!(
            t.contains("-S vcodec:h264,acodec:aac --merge-output-format mp4"),
            "{t}"
        );
        assert!(t.contains("--embed-thumbnail --embed-metadata --embed-chapters"));
    }

    #[test]
    fn quality_format_and_extras_reach_ytdlp() {
        let opts = DownloadOptions {
            max_height: Some(720),
            container: DownloadContainer::Mkv,
            playlist: true,
            subtitles: true,
            subtitle_langs: "en,es; rm -rf /".to_owned(),
            rate_limit_kbps: Some(500),
            ..DownloadOptions::default()
        };
        let t = build(&opts);
        assert!(
            t.contains("-f bv*+ba/b -S res:720 --merge-output-format mkv"),
            "{t}"
        );
        assert!(t.contains("--yes-playlist"));
        assert!(t.contains("%(playlist_index)03d"));
        // Only language characters survive.
        assert!(t.contains("--sub-langs en,esrm-rf --embed-subs"), "{t}");
        assert!(t.contains("--limit-rate 500K"));
    }

    #[test]
    fn audio_downloads_extract_and_skip_cover_for_wav() {
        let mut opts = DownloadOptions {
            kind: DownloadKind::Audio,
            audio_format: DownloadAudio::Flac,
            subtitles: true,
            ..DownloadOptions::default()
        };
        let t = build(&opts);
        assert!(t.contains("-x --audio-format flac --audio-quality 0"));
        assert!(t.contains("--embed-thumbnail"));
        assert!(!t.contains("--embed-subs") && !t.contains("--embed-chapters"));
        opts.audio_format = DownloadAudio::Wav;
        assert!(!build(&opts).contains("--embed-thumbnail"));
    }

    #[test]
    fn progress_lines_become_words() {
        let p = parse_progress("FMX|12400000|120000000|NA|2300000|45|NA|NA").expect("line");
        assert_eq!(p.fraction, Some(12.4 / 120.0));
        assert!(p.detail.contains("12.4 MB") && p.detail.contains("120.0 MB"));
        assert!(p.detail.contains("2.3 MB/s") && p.detail.contains("0:45"));

        // Playlist item 3 of 4, half done: 2.5 of 4 overall.
        let p = parse_progress("FMX|50|NA|100|NA|NA|3|4").expect("line");
        assert_eq!(p.fraction, Some(0.625));
        assert!(p.detail.contains('3') && p.detail.contains('4'));

        let p = parse_progress("[Merger] Merging formats into \"x.mp4\"").expect("step");
        assert_eq!(p.fraction, None);
        assert!(parse_progress("[youtube] abc: Downloading webpage").is_none());
        assert!(parse_progress("FMX|NA|NA|NA|NA|NA|NA|NA").is_none());
    }

    #[test]
    fn link_details_are_read() {
        let json = r#"{"title":"Me at the zoo","channel":"jawed","extractor_key":"Youtube",
            "duration":19,"thumbnail":"https://i.ytimg.com/vi/x/hq.jpg",
            "formats":[{"height":240,"vcodec":"avc1"},{"height":null,"vcodec":"none"},
            {"height":360,"vcodec":"vp9"}]}"#;
        let info = parse_info(json).expect("info");
        assert_eq!(info.title, "Me at the zoo");
        assert_eq!(info.channel.as_deref(), Some("jawed"));
        assert_eq!(info.max_height, Some(360));
        assert_eq!(info.playlist_count, None);

        let list = r#"{"_type":"playlist","title":"Mix","entries":[{},{},{}],
            "thumbnail":"http://insecure/x.jpg"}"#;
        let info = parse_info(list).expect("playlist");
        assert_eq!(info.playlist_count, Some(3));
        assert_eq!(info.thumbnail, None);
    }
}
