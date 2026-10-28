//! The "Convert with FMX" entry in file managers' right-click menus.
//!
//! Right-clicking files offers the conversions that fit them (video to MP4,
//! images to PDF, …), each of which runs `fmx --convert <target> <files>`:
//! the files are queued with the current settings and converted right away.
//! "Open in FMX" opens them in FMX instead.
//!
//! FMX writes the entries itself, per user, so installers and the Settings
//! switch share one source of truth:
//! - Windows: `HKCU\Software\Classes\SystemFileAssociations\.<ext>\shell`,
//!   written through the registry API (Windows 11 lists it under "Show more options");
//! - Linux: Dolphin service menus, Nemo actions, and Nautilus and Caja scripts.
//!
//! The entries name this executable and are in the UI language, so
//! [`refresh`] rewrites them when either changes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::catalog::{self, AudioFormat, EbookFormat, ImageFormat, Tool, VideoFormat};
use crate::i18n::{tr, trf};

/// Lists what was written by the last registration, and for which program
/// and language; missing when the menu is off.
const MARKER: &str = "shell-menu";
/// The command-line switch the menu entries use.
pub const CONVERT_FLAG: &str = "--convert";

/// A kind of file and the conversions its menu offers.
struct Group {
    /// Names the group's Linux menu files.
    #[cfg_attr(windows, allow(dead_code))]
    id: &'static str,
    extensions: &'static [&'static str],
    /// MIME types, for Linux file managers that filter by them.
    #[cfg_attr(windows, allow(dead_code))]
    mime_types: &'static [&'static str],
    targets: &'static [(&'static str, Tool)],
}

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "flv", "wmv", "mpg", "mpeg", "ts", "mts", "m2ts",
    "ogv", "3gp", "vob", "rmvb", "rm", "asf",
];
/// Office formats only: plain text, Markdown and HTML files are too common to
/// give all of them a conversion menu.
const OFFICE_EXTENSIONS: &[&str] = &[
    "docx", "doc", "odt", "rtf", "xlsx", "xls", "ods", "pptx", "ppt", "odp",
];

const GROUPS: &[Group] = &[
    Group {
        id: "video",
        extensions: VIDEO_EXTENSIONS,
        mime_types: &["video/*"],
        targets: &[
            ("mp4", Tool::ToVideo(VideoFormat::Mp4)),
            ("mkv", Tool::ToVideo(VideoFormat::Mkv)),
            ("webm", Tool::ToVideo(VideoFormat::Webm)),
            ("gif", Tool::ToVideo(VideoFormat::Gif)),
            ("mp3", Tool::ToAudio(AudioFormat::Mp3)),
        ],
    },
    Group {
        id: "audio",
        extensions: catalog::AUDIO_EXTENSIONS,
        mime_types: &["audio/*"],
        targets: &[
            ("mp3", Tool::ToAudio(AudioFormat::Mp3)),
            ("m4a", Tool::ToAudio(AudioFormat::M4a)),
            ("flac", Tool::ToAudio(AudioFormat::Flac)),
            ("wav", Tool::ToAudio(AudioFormat::Wav)),
            ("opus", Tool::ToAudio(AudioFormat::Opus)),
        ],
    },
    Group {
        id: "image",
        extensions: &[
            "png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif", "ico", "tga", "heic",
            "heif", "avif", "jxl",
        ],
        mime_types: &["image/*"],
        targets: &[
            ("jpg", Tool::ToImage(ImageFormat::Jpeg)),
            ("png", Tool::ToImage(ImageFormat::Png)),
            ("webp", Tool::ToImage(ImageFormat::Webp)),
            ("pdf", Tool::ImagesToPdf),
        ],
    },
    Group {
        id: "office",
        extensions: OFFICE_EXTENSIONS,
        mime_types: &[
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/msword",
            "application/vnd.oasis.opendocument.text",
            "application/rtf",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "application/vnd.ms-excel",
            "application/vnd.oasis.opendocument.spreadsheet",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "application/vnd.ms-powerpoint",
            "application/vnd.oasis.opendocument.presentation",
        ],
        targets: &[("pdf", Tool::OfficeToPdf)],
    },
    Group {
        id: "ebook",
        extensions: &["epub", "fb2"],
        mime_types: &["application/epub+zip", "application/x-fictionbook+xml"],
        targets: &[
            ("pdf", Tool::ToEbook(EbookFormat::Pdf)),
            ("txt", Tool::ToEbook(EbookFormat::Txt)),
            ("epub", Tool::ToEbook(EbookFormat::Epub)),
            ("fb2", Tool::ToEbook(EbookFormat::Fb2)),
        ],
    },
    Group {
        id: "pdf",
        extensions: &["pdf"],
        mime_types: &["application/pdf"],
        targets: &[
            ("word", Tool::PdfToWord),
            ("txt", Tool::PdfToText),
            ("images", Tool::PdfToImages),
        ],
    },
];

fn group_of(path: &Path) -> Option<&'static Group> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    GROUPS.iter().find(|g| g.extensions.contains(&ext.as_str()))
}

/// The first conversion offered for `path`'s kind: the tool "Open in FMX" shows it in.
pub fn default_tool(path: &Path) -> Option<Tool> {
    group_of(path)?.targets.first().map(|(_, tool)| *tool)
}

/// The tool a menu `target` (such as `mp4` or `pdf`) means for `path`, if it fits it.
pub fn tool_for(target: &str, path: &Path) -> Option<Tool> {
    group_of(path)?
        .targets
        .iter()
        .find(|(id, _)| *id == target)
        .map(|(_, tool)| *tool)
}

/// A UI string for a menu or file name: without the RIGHT-TO-LEFT MARK that
/// right-to-left UI strings start with, which file managers lay out themselves.
fn menu_text(key: &'static str) -> &'static str {
    tr(key).trim_start_matches(crate::i18n::RTL_MARK)
}

/// What a menu item says, e.g. "To MP4".
fn item_label(target: &str) -> String {
    let format = match target {
        "mp4" => "MP4",
        "mkv" => "MKV",
        "webm" => "WebM",
        "gif" => "GIF",
        "mp3" => "MP3",
        "m4a" => "M4A",
        "flac" => "FLAC",
        "wav" => "WAV",
        "opus" => "Opus",
        "jpg" => "JPG",
        "png" => "PNG",
        "webp" => "WebP",
        "pdf" => "PDF",
        "word" => "Word",
        "txt" => "TXT",
        "epub" => "EPUB",
        "fb2" => "FB2",
        "images" => menu_text("shell.images"),
        other => other,
    };
    trf("shell.to", &[("format", &format)])
        .trim_start_matches(crate::i18n::RTL_MARK)
        .to_owned()
}

/// Every target, once, in menu order: for file managers whose menus can't
/// depend on the file type.
#[cfg_attr(windows, allow(dead_code))]
fn all_targets() -> Vec<&'static str> {
    let mut seen = Vec::new();
    for (id, _) in GROUPS.iter().flat_map(|g| g.targets) {
        if !seen.contains(id) {
            seen.push(*id);
        }
    }
    seen
}

/// Whether this system has a menu FMX can add to.
pub fn supported() -> bool {
    if cfg!(windows) {
        return true;
    }
    // A snap's home folder is its own, which file managers don't read.
    cfg!(target_os = "linux") && std::env::var_os("SNAP").is_none()
}

fn marker_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("fmx").join(MARKER))
}

/// Whether the menu entries are installed.
pub fn is_registered() -> bool {
    marker_path().is_some_and(|p| p.exists())
}

/// What the entries depend on; when it changes they are written again.
fn signature(exe: &Path) -> String {
    format!(
        "{} {} {}",
        crate::update::VERSION,
        crate::i18n::active_language(),
        exe.display()
    )
}

/// Adds (or rewrites) the menu entries for this executable.
pub fn register() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    unregister()?;
    let written = platform::register(&exe)?;
    let marker = marker_path().ok_or_else(|| io::Error::other("no data folder"))?;
    fs::create_dir_all(marker.parent().expect("marker has a folder"))?;
    let mut contents = signature(&exe);
    for path in written {
        contents.push('\n');
        contents.push_str(&path.to_string_lossy());
    }
    fs::write(marker, contents)?;
    tracing::info!(exe = %exe.display(), "right-click menu added");
    Ok(())
}

/// Removes the menu entries.
pub fn unregister() -> io::Result<()> {
    let Some(marker) = marker_path() else {
        return Ok(());
    };
    let written: Vec<PathBuf> = fs::read_to_string(&marker)
        .map(|s| s.lines().skip(1).map(PathBuf::from).collect())
        .unwrap_or_default();
    platform::unregister(&written)?;
    match fs::remove_file(&marker) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    tracing::info!("right-click menu removed");
    Ok(())
}

/// Rewrites the entries if FMX moved, was updated, or speaks another language.
pub fn refresh() {
    let (Some(marker), Ok(exe)) = (marker_path(), std::env::current_exe()) else {
        return;
    };
    let Ok(contents) = fs::read_to_string(marker) else {
        return;
    };
    if contents.lines().next() != Some(signature(&exe).as_str())
        && let Err(e) = register()
    {
        tracing::warn!(error = %e, "could not update the right-click menu");
    }
}

#[cfg(windows)]
mod platform {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    use super::*;

    /// Per-user file-type menus; no administrator rights needed.
    const ROOT: &str = r"Software\Classes\SystemFileAssociations";
    const VERB: &str = "FMX.Convert";

    fn verb_key(ext: &str) -> String {
        format!(r"{ROOT}\.{ext}\shell\{VERB}")
    }

    pub fn register(exe: &Path) -> io::Result<Vec<PathBuf>> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let exe = exe.display().to_string();
        let icon = format!("{exe},0");
        for group in GROUPS {
            // Numbered keys keep the menu in this order.
            let items: Vec<(String, String)> = group
                .targets
                .iter()
                .map(|(id, _)| {
                    (
                        item_label(id),
                        format!("\"{exe}\" {CONVERT_FLAG} {id} \"%1\""),
                    )
                })
                .chain(std::iter::once((
                    menu_text("shell.open").to_owned(),
                    format!("\"{exe}\" \"%1\""),
                )))
                .collect();
            for ext in group.extensions {
                let (menu, _) = hkcu.create_subkey(verb_key(ext))?;
                menu.set_value("MUIVerb", &menu_text("shell.menu"))?;
                menu.set_value("Icon", &icon)?;
                // An empty SubCommands makes the entry a submenu of the keys under `shell`.
                menu.set_value("SubCommands", &"")?;
                for (i, (label, command)) in items.iter().enumerate() {
                    let (item, _) = menu.create_subkey(format!(r"shell\{i:02}"))?;
                    item.set_value("MUIVerb", label)?;
                    item.set_value("Icon", &icon)?;
                    let (run, _) = item.create_subkey("command")?;
                    run.set_value("", command)?;
                }
            }
        }
        // Registry keys are removed by name, not from this list.
        Ok(Vec::new())
    }

    pub fn unregister(_written: &[PathBuf]) -> io::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for ext in GROUPS.iter().flat_map(|g| g.extensions) {
            match hkcu.delete_subkey_all(verb_key(ext)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    /// Quotes `exe` for a desktop-entry `Exec` line.
    fn desktop_exec(exe: &Path) -> String {
        let escaped = exe
            .display()
            .to_string()
            .replace('\\', r"\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$");
        format!("\"{escaped}\"")
    }

    /// Quotes `exe` for a POSIX shell script.
    fn shell_quote(exe: &Path) -> String {
        format!("'{}'", exe.display().to_string().replace('\'', r"'\''"))
    }

    fn write(
        path: PathBuf,
        contents: &str,
        executable: bool,
        out: &mut Vec<PathBuf>,
    ) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, contents)?;
        if executable {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        }
        out.push(path);
        Ok(())
    }

    /// A file-name-safe version of a menu label.
    fn file_name(label: &str) -> String {
        label.replace(['/', '\0'], "-")
    }

    pub fn register(exe: &Path) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        let (Some(data), Some(config)) = (dirs::data_dir(), dirs::config_dir()) else {
            return Ok(out);
        };
        let exec = desktop_exec(exe);
        let menu = menu_text("shell.menu");

        // Dolphin (KDE 6 and 5): one service menu per file kind, merged into one submenu.
        for group in GROUPS {
            let mut text = format!(
                "[Desktop Entry]\nType=Service\nMimeType={};\nX-KDE-Submenu={menu}\nIcon=fmx\nActions={};open;\n",
                group.mime_types.join(";"),
                group
                    .targets
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>()
                    .join(";"),
            );
            for (id, _) in group.targets {
                text.push_str(&format!(
                    "\n[Desktop Action {id}]\nName={}\nIcon=fmx\nExec={exec} {CONVERT_FLAG} {id} %F\n",
                    item_label(id)
                ));
            }
            text.push_str(&format!(
                "\n[Desktop Action open]\nName={}\nIcon=fmx\nExec={exec} %F\n",
                menu_text("shell.open")
            ));
            for dir in ["kio/servicemenus", "kservices5/ServiceMenus"] {
                let path = data.join(dir).join(format!("fmx-{}.desktop", group.id));
                write(path, &text, true, &mut out)?;
            }
        }

        // Nemo (Cinnamon): one action per conversion, shown only for fitting files.
        for group in GROUPS {
            for (id, _) in group.targets {
                let text = format!(
                    "[Nemo Action]\nName={menu}: {}\nIcon-Name=fmx\nExec={exec} {CONVERT_FLAG} {id} %F\nSelection=notnone\nExtensions={};\n",
                    item_label(id),
                    group.extensions.join(";"),
                );
                let path = data
                    .join("nemo/actions")
                    .join(format!("fmx-{}-{id}.nemo_action", group.id));
                write(path, &text, false, &mut out)?;
            }
        }

        // Nautilus and Caja: a scripts submenu. Scripts show for every file, so
        // FMX skips the files a conversion doesn't fit.
        let exe = shell_quote(exe);
        for scripts in [data.join("nautilus/scripts"), config.join("caja/scripts")] {
            let folder = scripts.join(file_name(menu));
            for id in all_targets() {
                let text = format!("#!/bin/sh\nexec {exe} {CONVERT_FLAG} {id} \"$@\"\n");
                write(
                    folder.join(file_name(&item_label(id))),
                    &text,
                    true,
                    &mut out,
                )?;
            }
            let text = format!("#!/bin/sh\nexec {exe} \"$@\"\n");
            write(
                folder.join(file_name(menu_text("shell.open"))),
                &text,
                true,
                &mut out,
            )?;
            out.push(folder);
        }
        Ok(out)
    }

    pub fn unregister(written: &[PathBuf]) -> io::Result<()> {
        // Files first, then the (now empty) script folders.
        for path in written {
            if path.is_file() {
                fs::remove_file(path)?;
            }
        }
        for path in written {
            if path.is_dir() {
                let _ = fs::remove_dir(path);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_fit_the_file_type() {
        let t = |target, file: &str| tool_for(target, Path::new(file));
        assert_eq!(t("mp4", "clip.MOV"), Some(Tool::ToVideo(VideoFormat::Mp4)));
        assert_eq!(t("mp3", "clip.mkv"), Some(Tool::ToAudio(AudioFormat::Mp3)));
        assert_eq!(t("mp3", "song.flac"), Some(Tool::ToAudio(AudioFormat::Mp3)));
        assert_eq!(t("pdf", "scan.jpg"), Some(Tool::ImagesToPdf));
        assert_eq!(t("pdf", "report.docx"), Some(Tool::OfficeToPdf));
        assert_eq!(t("word", "report.pdf"), Some(Tool::PdfToWord));
        assert_eq!(
            t("pdf", "novel.epub"),
            Some(Tool::ToEbook(EbookFormat::Pdf))
        );
        assert_eq!(t("mp4", "report.pdf"), None);
        assert_eq!(t("mp4", "no-extension"), None);
    }

    /// Every conversion a menu offers must accept every file that menu shows for.
    #[test]
    fn every_target_tool_accepts_its_group() {
        for group in GROUPS {
            for (id, tool) in group.targets {
                for ext in group.extensions {
                    assert!(
                        tool.input_kind().extensions().contains(ext),
                        "{id} does not take .{ext}"
                    );
                }
            }
        }
    }

    #[test]
    fn file_kinds_do_not_overlap() {
        let mut seen = std::collections::HashSet::new();
        for ext in GROUPS.iter().flat_map(|g| g.extensions) {
            assert!(seen.insert(ext), ".{ext} is in two menus");
        }
    }
}
