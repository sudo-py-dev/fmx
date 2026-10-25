//! Argument builders for the document, eBook, ISO and download tools, which
//! delegate to Poppler, Ghostscript, LibreOffice, Calibre, xorriso and yt-dlp.
//! Pure functions, so every command line is unit-testable.

use std::ffi::OsString;
use std::path::Path;

use crate::catalog::ImageFormat;
use crate::options::Quality;

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// printf-style tools treat `%` in paths as a format directive.
fn escape_percent(path: &Path) -> OsString {
    path.to_string_lossy().replace('%', "%%").into()
}

/// `pdftoppm`: one image per page into `out_dir/page-N.ext`.
pub fn pdf_to_images(input: &Path, out_dir: &Path, dpi: u32, format: ImageFormat) -> Vec<OsString> {
    let kind = if format == ImageFormat::Jpeg {
        "-jpeg"
    } else {
        "-png"
    };
    let mut args = os(&["-r", &dpi.to_string(), kind]);
    args.push(input.into());
    args.push(out_dir.join("page").into());
    args
}

/// `pdftotext`, keeping the page layout.
pub fn pdf_to_text(input: &Path, output: &Path) -> Vec<OsString> {
    let mut args = os(&["-layout", "-enc", "UTF-8"]);
    args.extend([input.into(), output.into()]);
    args
}

/// `pdftohtml` as one self-contained page.
pub fn pdf_to_html(input: &Path, output: &Path) -> Vec<OsString> {
    let mut args = os(&["-s", "-noframes", "-q", "-enc", "UTF-8"]);
    args.extend([input.into(), output.into()]);
    args
}

/// `pdfunite in1 in2 … out`.
pub fn pdf_merge(inputs: &[impl AsRef<Path>], output: &Path) -> Vec<OsString> {
    inputs
        .iter()
        .map(|p| p.as_ref().into())
        .chain([output.into()])
        .collect()
}

/// `pdfseparate in out_dir/page-%d.pdf`.
pub fn pdf_split(input: &Path, out_dir: &Path) -> Vec<OsString> {
    let mut pattern = escape_percent(out_dir);
    pattern.push(format!("{}page-%d.pdf", std::path::MAIN_SEPARATOR));
    vec![input.into(), pattern]
}

/// Ghostscript re-distilling a PDF with image downsampling.
pub fn pdf_compress(input: &Path, output: &Path, quality: Quality) -> Vec<OsString> {
    let preset = quality.pick("/printer", "/ebook", "/screen");
    let mut out_flag = OsString::from("-sOutputFile=");
    out_flag.push(escape_percent(output));
    let mut args = os(&[
        "-sDEVICE=pdfwrite",
        "-dCompatibilityLevel=1.5",
        &format!("-dPDFSETTINGS={preset}"),
        "-dNOPAUSE",
        "-dQUIET",
        "-dBATCH",
        "-dSAFER",
    ]);
    args.push(out_flag);
    // `--` ends option parsing so the input is always treated as a file.
    args.push("--".into());
    args.push(input.into());
    args
}

/// LibreOffice headless conversion to PDF into `out_dir`, isolated in its own
/// profile so it never collides with a running office instance.
pub fn office_to_pdf(input: &Path, out_dir: &Path, profile_dir: &Path) -> Vec<OsString> {
    let mut profile = OsString::from("-env:UserInstallation=file://");
    profile.push(profile_dir.as_os_str());
    let mut args = vec![profile];
    args.extend(os(&["--headless", "--norestore", "--convert-to", "pdf"]));
    args.extend(["--outdir".into(), out_dir.into(), input.into()]);
    args
}

/// Calibre's `ebook-convert in out`; the output extension selects the format.
pub fn ebook_convert(input: &Path, output: &Path) -> Vec<OsString> {
    vec![input.into(), output.into()]
}

/// ISO 9660 volume labels allow A–Z, 0–9 and `_`, up to 32 characters.
pub fn volume_label(text: &str) -> String {
    let label: String = text
        .chars()
        .map(|c| c.to_ascii_uppercase())
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(32)
        .collect();
    if label.is_empty() {
        "DISC".to_owned()
    } else {
        label
    }
}

/// `xorriso -as mkisofs`, adding DVD-Video layout when the folder has `VIDEO_TS`.
pub fn folder_to_iso(
    folder: &Path,
    output: &Path,
    label: &str,
    is_dvd_video: bool,
) -> Vec<OsString> {
    let mut args = os(&[
        "-as",
        "mkisofs",
        "-r",
        "-J",
        "-joliet-long",
        "-V",
        &volume_label(label),
    ]);
    if is_dvd_video {
        args.push("-dvd-video".into());
    }
    // mkisofs emulation has no `--`; the absolute folder path cannot look like an option.
    args.extend(["-o".into(), output.into(), folder.into()]);
    args
}

/// Parses xorriso's `xorriso : UPDATE : 12.34% done` progress lines.
pub fn xorriso_progress(line: &str) -> Option<f32> {
    let (before, _) = line.split_once("% done")?;
    let number = before.rsplit(|c: char| c.is_whitespace()).next()?;
    number
        .parse::<f32>()
        .ok()
        .map(|p| (p / 100.0).clamp(0.0, 1.0))
}

/// Only plain web addresses are passed to yt-dlp (no search terms or local files).
pub fn is_web_url(text: &str) -> bool {
    let text = text.trim();
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"));
    rest.is_some_and(|r| {
        !r.is_empty() && !r.starts_with('/') && !text.contains(char::is_whitespace)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(args: &[OsString]) -> String {
        args.iter()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn ghostscript_escapes_percent_and_ends_options() {
        let args = pdf_compress(
            Path::new("/in/a.pdf"),
            Path::new("/o/100%.pdf"),
            Quality::Low,
        );
        let t = text(&args);
        assert!(t.contains("-dPDFSETTINGS=/screen"));
        assert!(t.contains("-sOutputFile=/o/100%%.pdf -- /in/a.pdf"));
    }

    #[test]
    fn pdf_split_pattern() {
        let t = text(&pdf_split(Path::new("/a.pdf"), Path::new("/o/x%y")));
        // Built with join so the separator matches the platform.
        let pattern = Path::new("/o/x%%y").join("page-%d.pdf");
        assert!(t.ends_with(&*pattern.to_string_lossy()), "{t}");
    }

    #[test]
    fn office_uses_isolated_profile() {
        let t = text(&office_to_pdf(
            Path::new("/a.doc"),
            Path::new("/o"),
            Path::new("/tmp/p"),
        ));
        assert!(t.starts_with("-env:UserInstallation=file:///tmp/p --headless"));
        assert!(t.ends_with("--convert-to pdf --outdir /o /a.doc"));
    }

    #[test]
    fn labels_are_sanitized() {
        assert_eq!(volume_label("my disc-1"), "MY_DISC_1");
        assert_eq!(volume_label(""), "DISC");
        assert_eq!(volume_label(&"x".repeat(40)).len(), 32);
        let t = text(&folder_to_iso(
            Path::new("/f"),
            Path::new("/o.iso"),
            "dvd",
            true,
        ));
        assert!(t.ends_with("-V DVD -dvd-video -o /o.iso /f"));
    }

    #[test]
    fn xorriso_progress_is_read() {
        assert_eq!(
            xorriso_progress("xorriso : UPDATE :  45.50% done"),
            Some(0.455)
        );
        assert_eq!(xorriso_progress("xorriso : NOTE : nothing"), None);
    }

    #[test]
    fn only_web_urls_are_accepted() {
        assert!(is_web_url("https://example.com/watch?v=1"));
        assert!(!is_web_url("ytsearch:cats"));
        assert!(!is_web_url("/etc/passwd"));
        assert!(!is_web_url("https://"));
        assert!(!is_web_url("https://a.com/x y"));
        assert!(!is_web_url("file:///etc/passwd"));
    }
}
