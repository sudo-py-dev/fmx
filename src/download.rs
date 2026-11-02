//! The video downloader: yt-dlp's command line, its progress lines, and the
//! details shown for a link before downloading.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
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
const MAX_COOKIES_BYTES: u64 = 4 << 20;
// Keep both entry points isolated from local executable plugin/config content.
const ISOLATION_ARGS: &[&str] = &[
    "--ignore-config",
    "--no-plugin-dirs",
    "--no-remote-components",
];

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// The yt-dlp arguments that download `url` into `out_dir` as `opts` asks,
/// using our ffmpeg for merging and conversion.
pub fn args(
    url: &str,
    out_dir: &Path,
    ffmpeg: &Path,
    opts: &DownloadOptions,
) -> Result<Vec<OsString>, &'static str> {
    if !crate::docs::is_web_url(url) {
        return Err("error.bad_url");
    }
    validate_fields(opts)?;
    let access = access_args(&opts.cookies_file)?;
    let out_text = out_dir
        .to_str()
        .filter(|s| !s.is_empty() && !s.contains('$') && !s.chars().any(char::is_control))
        .ok_or("section.output_settings")?;
    // Output paths are data, even when a folder contains yt-dlp template syntax.
    let out_dir =
        std::path::absolute(Path::new(out_text)).map_err(|_| "section.output_settings")?;
    let escaped_out = out_dir
        .to_str()
        .ok_or("section.output_settings")?
        .replace('%', "%%");
    let name = if opts.playlist {
        "%(playlist_title,title).100B/%(playlist_index)03d - %(title).150B [%(id)s].%(ext)s"
    } else {
        "%(title).150B [%(id)s].%(ext)s"
    };
    let mut template = OsString::from(&escaped_out);
    template.push(std::path::MAIN_SEPARATOR_STR);
    template.push(name);

    let mut args = os(ISOLATION_ARGS);
    args.extend(os(&[
        // User and working-directory configs may contain --exec or override
        // output paths. Only FMX's explicit options should control this child.
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
    ]));
    args.extend([
        "--ffmpeg-location".into(),
        ffmpeg.into(),
        "-o".into(),
        template,
    ]);
    let video = opts.kind == DownloadKind::Video;
    let media = matches!(opts.kind, DownloadKind::Video | DownloadKind::Audio);
    if !media {
        args.extend(os(&["--skip-download"]));
    }
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
    } else if opts.kind == DownloadKind::Audio {
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
    let wants_subs = (video && opts.subtitles)
        || (media && opts.save_subtitles)
        || opts.kind == DownloadKind::Subtitles;
    if wants_subs {
        args.push(
            format!(
                "--sub-langs={}",
                subtitle_langs(&opts.subtitle_langs).ok_or("download.subtitles_label")?
            )
            .into(),
        );
        if video && opts.subtitles {
            args.extend(os(&["--embed-subs"]));
        }
        if opts.save_subtitles || opts.kind == DownloadKind::Subtitles {
            args.extend(os(&["--write-subs"]));
        }
        if opts.auto_subtitles {
            args.extend(os(&["--write-auto-subs"]));
        }
    }
    if (media && opts.save_thumbnail) || opts.kind == DownloadKind::Thumbnail {
        args.extend(os(&["--write-thumbnail"]));
    }
    if (media && opts.save_info_json) || opts.kind == DownloadKind::Metadata {
        args.extend(os(&["--write-info-json"]));
    }
    if (media && opts.save_description) || opts.kind == DownloadKind::Metadata {
        args.extend(os(&["--write-description"]));
    }
    if opts.playlist && !opts.playlist_items.trim().is_empty() {
        args.push(format!("--playlist-items={}", opts.playlist_items.trim()).into());
    }
    if media {
        if opts.clip {
            let end = opts
                .end_seconds
                .map_or_else(|| "inf".to_owned(), |s| s.to_string());
            args.extend(os(&[
                "--download-sections",
                &format!("*{}-{end}", opts.start_seconds),
            ]));
        }
        if opts.split_chapters {
            // Explicit absolute chapter output prevents yt-dlp's default from
            // writing split files into the application's working directory.
            let chapter = Path::new(&escaped_out).join(
                "%(title).150B [%(id)s] - %(section_number)03d - %(section_title).100B.%(ext)s",
            );
            let mut template = OsString::from("chapter:");
            template.push(chapter);
            args.extend(["--split-chapters".into(), "-o".into(), template]);
        }
        if opts.remove_sponsors {
            args.extend(os(&["--sponsorblock-remove", "sponsor"]));
        }
        if opts.live_from_start {
            args.extend(os(&["--live-from-start"]));
        }
        if opts.remember_downloads {
            // Separate histories avoid skipping audio after downloading video,
            // or treating a saved clip as a completed full video.
            let range = if opts.clip {
                format!(
                    "{}-{}",
                    opts.start_seconds,
                    opts.end_seconds
                        .map_or_else(|| "end".to_owned(), |n| n.to_string())
                )
            } else {
                "full".to_owned()
            };
            let archive = format!(
                ".fmx-archive-{:?}-{}-{}-{}-{}-{}.txt",
                opts.kind,
                if video {
                    opts.container.ext()
                } else {
                    opts.audio_format.ext()
                },
                if video {
                    opts.max_height.unwrap_or(0)
                } else {
                    0
                },
                range,
                opts.split_chapters,
                opts.remove_sponsors
            );
            args.extend([
                "--download-archive".into(),
                out_dir.join(archive).into_os_string(),
            ]);
        }
    }
    args.extend(os(&[
        "--concurrent-fragments",
        &opts.concurrent_fragments.clamp(1, 16).to_string(),
        "--retries",
        &opts.retries.min(100).to_string(),
        "--fragment-retries",
        &opts.retries.min(100).to_string(),
    ]));
    args.extend(access);
    if opts.embed_thumbnail && supports_cover(opts) {
        args.extend(os(&["--embed-thumbnail"]));
    }
    if media && opts.embed_metadata {
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
    Ok(args)
}

/// Shared by link lookup and download, so authenticated links can be previewed.
fn access_args(cookies_file: &str) -> Result<Vec<OsString>, &'static str> {
    match validated_cookies(cookies_file)? {
        None => Ok(Vec::new()),
        Some(path) => {
            // Bind the value in one argv entry: even a filename beginning with
            // a dash cannot become an option. Command::args never uses a shell.
            let mut arg = OsString::from("--cookies=");
            arg.push(path);
            Ok(vec![arg])
        }
    }
}

fn validated_cookies(text: &str) -> Result<Option<PathBuf>, &'static str> {
    let invalid = "download.cookies";
    if text.len() > 4096 || text.chars().any(char::is_control) {
        return Err(invalid);
    }
    if text.trim().is_empty() {
        return Ok(None);
    }
    let path = Path::new(text.trim()).canonicalize().map_err(|_| invalid)?;
    // yt-dlp expands environment variables in cookie filenames. Reject the
    // expansion syntax so it opens exactly the file that was validated here.
    let path_text = path.to_str().ok_or(invalid)?;
    if path_text.contains('$') || (cfg!(windows) && path_text.contains('%')) {
        return Err(invalid);
    }
    let meta = path.metadata().map_err(|_| invalid)?;
    if !meta.is_file() || meta.len() > MAX_COOKIES_BYTES {
        return Err(invalid);
    }
    let file = std::fs::File::open(&path).map_err(|_| invalid)?;
    let meta = file.metadata().map_err(|_| invalid)?;
    if !meta.is_file() || meta.len() > MAX_COOKIES_BYTES {
        return Err(invalid);
    }
    let mut data = String::new();
    file.take(MAX_COOKIES_BYTES + 1)
        .read_to_string(&mut data)
        .map_err(|_| invalid)?;
    if data.len() as u64 > MAX_COOKIES_BYTES || !valid_cookie_data(&data) {
        return Err(invalid);
    }
    Ok(Some(path))
}

/// Netscape's seven tab-separated fields; never treat this as configuration.
fn valid_cookie_data(text: &str) -> bool {
    let mut lines = text.lines();
    if !matches!(
        lines.next(),
        Some("# Netscape HTTP Cookie File" | "# HTTP Cookie File")
    ) {
        return false;
    }
    lines.all(|line| {
        if line.is_empty() { return true; }
        if line.chars().any(|c| c.is_control() && c != '\t') { return false; }
        let line = if let Some(cookie) = line.strip_prefix("#HttpOnly_") { cookie }
            else if line.starts_with('#') { return true; } else { line };
        let f: Vec<_> = line.split('\t').collect();
        if f.len() != 7 { return false; }
        let domain = f[0].strip_prefix('.').unwrap_or(f[0]);
        let valid_domain = !domain.is_empty() && domain.len() <= 253 && domain.split('.').all(|part|
            !part.is_empty() && part.len() <= 63 && !part.starts_with('-') && !part.ends_with('-') &&
            part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'));
        valid_domain && matches!(f[1], "TRUE" | "FALSE") && f[2].starts_with('/') &&
            !f[2].chars().any(char::is_whitespace) && matches!(f[3], "TRUE" | "FALSE") &&
            (f[4].is_empty() || (f[4].bytes().all(|c| c.is_ascii_digit()) && f[4].parse::<u64>().is_ok())) &&
            // RFC cookie names are tokens; empty names are supported by Netscape.
            f[5].bytes().all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
    })
}

/// yt-dlp's comma-separated indices and START:STOP:STEP slices.
fn valid_playlist_items(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    if text.len() > 256 {
        return false;
    }
    text.split(',').all(|item| {
        let fields: Vec<_> = item.split(':').collect();
        !item.is_empty()
            && fields.len() <= 3
            && fields.iter().enumerate().all(|(i, n)| {
                if n.is_empty() {
                    return fields.len() > 1;
                }
                // ASCII-only: do not let plus signs or other yt-dlp expressions through.
                let digits = n.strip_prefix('-').unwrap_or(n);
                !digits.is_empty()
                    && digits.bytes().all(|c| c.is_ascii_digit())
                    && n.parse::<i32>()
                        .is_ok_and(|n| n != 0 || (fields.len() > 1 && i < 2))
            })
    })
}

/// Checks user-entered advanced values before a job is queued.
pub fn validate(opts: &DownloadOptions) -> Result<(), &'static str> {
    validate_fields(opts)?;
    validated_cookies(&opts.cookies_file)?;
    Ok(())
}

fn validate_fields(opts: &DownloadOptions) -> Result<(), &'static str> {
    if subtitle_langs(&opts.subtitle_langs).is_none() {
        return Err("download.subtitles_label");
    }
    if opts
        .max_height
        .is_some_and(|h| !crate::options::DOWNLOAD_HEIGHTS.contains(&h))
    {
        return Err("option.quality");
    }
    if opts
        .rate_limit_kbps
        .is_some_and(|n| n == 0 || n > crate::options::MAX_RATE_LIMIT_KBPS)
    {
        return Err("download.speed_label");
    }
    let items = opts.playlist_items.trim();
    if opts.playlist && !valid_playlist_items(items) {
        return Err("download.playlist_items");
    }
    if matches!(opts.kind, DownloadKind::Video | DownloadKind::Audio)
        && opts.clip
        && (opts.start_seconds > 604800
            || opts
                .end_seconds
                .is_some_and(|end| end <= opts.start_seconds || end > 604800))
    {
        return Err("download.clip");
    }
    if !(1..=16).contains(&opts.concurrent_fragments) || opts.retries > 100 {
        return Err("download.connection");
    }
    Ok(())
}

/// Whether the chosen format can hold cover art.
pub fn supports_cover(opts: &DownloadOptions) -> bool {
    match opts.kind {
        DownloadKind::Video => opts.container != DownloadContainer::Webm,
        DownloadKind::Audio => opts.audio_format != DownloadAudio::Wav,
        _ => false,
    }
}

/// Accept language tags, optional exclusions, and a single trailing wildcard.
/// Arbitrary regular expressions and command text are deliberately rejected.
fn subtitle_langs(text: &str) -> Option<String> {
    if text.len() > crate::options::MAX_SUBTITLE_LANGS_LEN || text.chars().any(char::is_control) {
        return None;
    }
    let mut result = Vec::new();
    for tag in text.split(',').map(str::trim) {
        if tag == "*" {
            result.push("all");
            continue;
        }
        let tag_value = tag.strip_prefix('-').unwrap_or(tag);
        let prefix = tag_value.strip_suffix(".*").unwrap_or(tag_value);
        if prefix.is_empty()
            || !prefix.as_bytes()[0].is_ascii_alphabetic()
            || !prefix
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            || prefix.ends_with(['-', '_'])
        {
            return None;
        }
        result.push(tag);
    }
    Some(result.join(","))
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

/// Turns yt-dlp's `-J` answer into the details FMX shows.
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
pub fn fetch_info(
    ytdlp: &Path,
    url: &str,
    playlist: bool,
    cookies_file: &str,
) -> Result<LinkInfo, String> {
    if !crate::docs::is_web_url(url) {
        return Err(tr("error.bad_url").to_owned());
    }
    let access = access_args(cookies_file)
        .map_err(|key| trf("error.invalid_value", &[("name", &tr(key))]))?;
    let mut child = command(ytdlp)
        .args(ISOLATION_ARGS)
        .args([
            "-J",
            "--flat-playlist",
            "--no-warnings",
            if playlist {
                "--yes-playlist"
            } else {
                "--no-playlist"
            },
        ])
        .args(access)
        .args(["--", url.trim()])
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
    // Drain stderr concurrently so a full error pipe cannot stall stdout.
    // Retain only a bounded error message, discarding the rest.
    let error_reader = std::thread::spawn(move || {
        let mut message = String::new();
        if let Some(mut err) = stderr {
            let _ = (&mut err).take(64 << 10).read_to_string(&mut message);
            let _ = std::io::copy(&mut err, &mut std::io::sink());
        }
        message
    });
    let started = Instant::now();
    let status = loop {
        if started.elapsed() > INFO_TIMEOUT {
            return Err(tr("download.info_timeout").to_owned());
        }
        if reader.is_finished()
            && let Some(status) = child.0.try_wait().map_err(|e| e.to_string())?
        {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let text = reader
        .join()
        .map_err(|_| "reader failed".to_owned())?
        .map_err(|e| e.to_string())?;
    if !status.success() {
        // Do not block on a pipe inherited by a helper's descendant.
        let message = if error_reader.is_finished() {
            error_reader.join().unwrap_or_default()
        } else {
            String::new()
        };
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

    const VALID_COOKIES: &str =
        "# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tTRUE\t0\tsession\tvalue\n";

    fn cookie_fixture() -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), VALID_COOKIES).unwrap();
        file
    }

    #[test]
    fn reject_command_text_and_invalid_options_at_the_launch_boundary() {
        let run = |opts: &DownloadOptions| {
            args(
                "https://example.com/v",
                Path::new("/out"),
                Path::new("/ffmpeg"),
                opts,
            )
        };
        for payload in [
            "en --exec=touch",
            "$(touch /tmp/sentinel)",
            "en;id",
            "en\n--exec=id",
            "en,(a+)+$",
            "",
            "en,,es",
            "--exec",
        ] {
            let opts = DownloadOptions {
                subtitle_langs: payload.into(),
                ..DownloadOptions::default()
            };
            assert_eq!(run(&opts), Err("download.subtitles_label"), "{payload}");
        }
        for valid in [
            "en,es",
            "en-US",
            "en.*,ja",
            "all,-live_chat",
            "*",
            " en, es ",
        ] {
            assert!(subtitle_langs(valid).is_some(), "{valid}");
        }
        for payload in ["--exec=id", "1;id", "1\n--exec=id", "1,,2", "1:2:0"] {
            let opts = DownloadOptions {
                playlist: true,
                playlist_items: payload.into(),
                ..DownloadOptions::default()
            };
            assert_eq!(run(&opts), Err("download.playlist_items"), "{payload}");
        }
        for opts in [
            DownloadOptions {
                max_height: Some(u32::MAX),
                ..DownloadOptions::default()
            },
            DownloadOptions {
                retries: 101,
                ..DownloadOptions::default()
            },
            DownloadOptions {
                concurrent_fragments: 0,
                ..DownloadOptions::default()
            },
            DownloadOptions {
                rate_limit_kbps: Some(0),
                ..DownloadOptions::default()
            },
        ] {
            assert!(run(&opts).is_err());
        }
    }

    #[test]
    fn cookie_files_must_be_bounded_regular_netscape_data() {
        for invalid in [
            "--exec=touch /tmp/sentinel",
            "{}",
            "# Netscape HTTP Cookie File\n--exec=id\n",
            "# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tTRUE\t0\tname\n",
            "# Netscape HTTP Cookie File\n.example.com\tMAYBE\t/\tTRUE\t0\tname\tvalue\n",
            "# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tTRUE\t$(id)\tname\tvalue\n",
            "# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tTRUE\t0\tname\tvalue\0\n",
        ] {
            let file = cookie_fixture();
            std::fs::write(file.path(), invalid).unwrap();
            assert_eq!(
                access_args(file.path().to_str().unwrap()),
                Err("download.cookies")
            );
        }
        let file = cookie_fixture();
        assert!(access_args(file.path().to_str().unwrap()).is_ok());
        // HTTP-only cookie rows and CRLF exports are valid.
        assert!(valid_cookie_data(
            &VALID_COOKIES
                .replace(".example", "#HttpOnly_.example")
                .replace('\n', "\r\n")
        ));
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            access_args(dir.path().to_str().unwrap()),
            Err("download.cookies")
        );
        file.as_file().set_len(MAX_COOKIES_BYTES + 1).unwrap();
        assert_eq!(
            access_args(file.path().to_str().unwrap()),
            Err("download.cookies")
        );
    }

    #[test]
    fn command_like_filenames_and_url_queries_remain_literal_data() {
        let dir = tempfile::tempdir().unwrap();
        let cookie = dir.path().join("--exec=echo command; (id).txt");
        std::fs::write(&cookie, VALID_COOKIES).unwrap();
        let opts = DownloadOptions {
            cookies_file: cookie.to_str().unwrap().into(),
            ..DownloadOptions::default()
        };
        let url = "https://example.com/watch?value=$(id)&option=--exec=echo";
        let a = args(
            url,
            &dir.path().join("%(title)s"),
            Path::new("/ffmpeg"),
            &opts,
        )
        .unwrap();
        let cookie_arg = a
            .iter()
            .find(|s| s.to_string_lossy().starts_with("--cookies="))
            .unwrap();
        assert_eq!(
            cookie_arg.to_string_lossy(),
            format!("--cookies={}", cookie.canonicalize().unwrap().display())
        );
        assert_eq!(
            &a[a.len() - 2..],
            &[OsString::from("--"), OsString::from(url)]
        );
        assert!(a.windows(2).any(|pair| {
            pair[0] == "-o"
                && pair[1]
                    .to_string_lossy()
                    .contains(&format!("%%(title)s{}", std::path::MAIN_SEPARATOR))
        }));
        assert!(!a.iter().any(|s| s == "--exec" || s == "--config-locations"));
        for flag in ISOLATION_ARGS {
            assert!(a.iter().any(|s| s == flag));
        }
    }

    #[test]
    fn reject_cookie_paths_that_ytdlp_would_expand() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["$HOME-cookies.txt", "${FMX_TEST_COOKIE_PATH}.txt"] {
            let path = dir.path().join(name);
            std::fs::write(&path, VALID_COOKIES).unwrap();
            assert_eq!(access_args(path.to_str().unwrap()), Err("download.cookies"));
        }
        assert_eq!(
            args(
                "https://example.com/v",
                &dir.path().join("$HOME-output"),
                Path::new("/ffmpeg"),
                &DownloadOptions::default()
            ),
            Err("section.output_settings")
        );
    }

    #[test]
    fn previews_reject_invalid_values_before_starting_a_process() {
        // An absent executable would produce a spawn error if validation ran too late.
        let missing = Path::new("/nonexistent/fmx-test-ytdlp");
        assert_eq!(
            fetch_info(missing, "--exec=id", false, ""),
            Err(tr("error.bad_url").to_owned())
        );
        let bad = cookie_fixture();
        std::fs::write(bad.path(), "--exec=id").unwrap();
        let error = fetch_info(
            missing,
            "https://example.com/v",
            false,
            bad.path().to_str().unwrap(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            trf("error.invalid_value", &[("name", &tr("download.cookies"))])
        );
        for url in [
            "--exec=id",
            "file:///etc/passwd",
            "https://example.com/\n--exec=id",
            "https://user:password@example.com/",
        ] {
            assert_eq!(
                args(
                    url,
                    Path::new("/o"),
                    Path::new("/ffmpeg"),
                    &DownloadOptions::default()
                ),
                Err("error.bad_url")
            );
        }
    }

    fn text(args: &[OsString]) -> String {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn build(opts: &DownloadOptions) -> String {
        text(
            &args(
                "https://e.com/v",
                Path::new("/o"),
                Path::new("/usr/bin/ffmpeg"),
                opts,
            )
            .expect("valid options"),
        )
    }

    #[test]
    fn options_end_before_the_link() {
        let t = build(&DownloadOptions::default());
        assert!(t.ends_with("-- https://e.com/v"));
        assert!(t.contains("--ignore-config"));
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
            subtitle_langs: "en,es".to_owned(),
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
        assert!(t.contains("--sub-langs=en,es --embed-subs"), "{t}");
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
    fn asset_modes_skip_media_and_ignore_hidden_media_options() {
        for kind in [
            DownloadKind::Subtitles,
            DownloadKind::Thumbnail,
            DownloadKind::Metadata,
        ] {
            let opts = DownloadOptions {
                kind,
                clip: true,
                split_chapters: true,
                remember_downloads: true,
                save_thumbnail: true,
                save_subtitles: true,
                save_info_json: true,
                ..DownloadOptions::default()
            };
            let t = build(&opts);
            assert!(t.contains("--skip-download"), "{t}");
            for flag in [
                "--download-sections",
                "--split-chapters",
                "--download-archive",
                "--embed-",
                "-x ",
                "-f ",
            ] {
                assert!(!t.contains(flag), "{flag}: {t}");
            }
            assert_eq!(t.contains("--write-subs"), kind == DownloadKind::Subtitles);
            assert_eq!(
                t.contains("--write-thumbnail"),
                kind == DownloadKind::Thumbnail
            );
            assert_eq!(
                t.contains("--write-info-json"),
                kind == DownloadKind::Metadata
            );
        }
    }

    #[test]
    fn advanced_download_options_and_paths_reach_ytdlp() {
        let cookie_file = cookie_fixture();
        let opts = DownloadOptions {
            playlist: true,
            playlist_items: "1:5,8".into(),
            clip: true,
            start_seconds: 30,
            end_seconds: Some(90),
            split_chapters: true,
            remove_sponsors: true,
            live_from_start: true,
            remember_downloads: true,
            save_subtitles: true,
            auto_subtitles: true,
            save_thumbnail: true,
            save_info_json: true,
            save_description: true,
            concurrent_fragments: 4,
            retries: 3,
            cookies_file: cookie_file.path().to_string_lossy().into_owned(),
            ..DownloadOptions::default()
        };
        let a = args(
            "https://e.com/v",
            Path::new("/out dir"),
            Path::new("/ffmpeg"),
            &opts,
        )
        .unwrap();
        let t = text(&a);
        for flag in [
            "--playlist-items=1:5,8",
            "--download-sections *30-90",
            "--split-chapters",
            "--sponsorblock-remove sponsor",
            "--live-from-start",
            "--write-auto-subs",
            "--write-subs",
            "--write-thumbnail",
            "--write-info-json",
            "--write-description",
            "--concurrent-fragments 4",
            "--retries 3",
            "--fragment-retries 3",
        ] {
            assert!(t.contains(flag), "{flag}: {t}");
        }
        assert!(a.iter().any(|a| a.to_string_lossy()
            == format!(
                "--cookies={}",
                cookie_file.path().canonicalize().unwrap().display()
            )));
        let chapter = a
            .iter()
            .find(|s| s.to_string_lossy().starts_with("chapter:"))
            .unwrap()
            .to_string_lossy();
        let output = std::path::absolute(Path::new("/out dir")).unwrap();
        assert!(chapter.starts_with(&format!(
            "chapter:{}{}",
            output.display(),
            std::path::MAIN_SEPARATOR
        )));
        let archive =
            a.windows(2).find(|a| a[0] == "--download-archive").unwrap()[1].to_string_lossy();
        assert!(archive.starts_with(&format!(
            "{}{}.fmx-archive-",
            output.display(),
            std::path::MAIN_SEPARATOR
        )));
        let mut other = opts.clone();
        other.kind = DownloadKind::Audio;
        assert_ne!(
            archive.as_ref(),
            args(
                "https://e.com/v",
                Path::new("/out dir"),
                Path::new("/ffmpeg"),
                &other
            )
            .unwrap()
            .windows(2)
            .find(|a| a[0] == "--download-archive")
            .unwrap()[1]
                .to_string_lossy()
        );
    }

    #[test]
    fn invalid_advanced_values_are_rejected_and_old_settings_load() {
        let mut opts: DownloadOptions = serde_json::from_str(r#"{"kind":"AudioMp3"}"#).unwrap();
        assert_eq!(opts.kind, DownloadKind::Audio);
        assert_eq!(validate(&opts), Ok(()));
        opts.clip = true;
        opts.start_seconds = 90;
        opts.end_seconds = Some(30);
        assert_eq!(validate(&opts), Err("download.clip"));
        opts.clip = false;
        opts.playlist = true;
        for valid in ["", "1:5,8", "-1", ":5", "1::2", "::-1"] {
            assert!(valid_playlist_items(valid), "{valid}");
        }
        for invalid in ["1,,2", "1:2:0", "0", "1:2:3:4", "--exec", "1;cmd"] {
            opts.playlist_items = invalid.into();
            assert_eq!(validate(&opts), Err("download.playlist_items"), "{invalid}");
        }
        opts.playlist = false;
        opts.cookies_file = "/definitely/missing/fmx-cookies.txt".into();
        assert_eq!(validate(&opts), Err("download.cookies"));
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
