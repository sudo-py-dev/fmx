//! Native (ffmpeg-free) image conversion and images-to-PDF.

use std::fmt;
use std::io::{BufWriter, Write};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageReader, RgbImage};

use crate::catalog::ImageFormat;
use crate::i18n::trf;
use crate::options::{self, Corner, JoinLayout, Options, Rotation};

/// ICO stores at most 256×256 pixels per image.
const ICO_MAX_SIDE: u32 = 256;
/// Decoder memory cap; guards against decompression bombs.
const MAX_DECODE_BYTES: u64 = 1 << 30;
/// PDF points per pixel when placing images (96 DPI screen pixels).
const POINTS_PER_PIXEL: f64 = 72.0 / 96.0;

#[derive(Debug)]
pub enum ImageError {
    Io(std::io::Error),
    Codec(image::ImageError),
    /// The format is written through ffmpeg, which failed or is missing.
    Encode(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => f.write_str(&trf("error.io", &[("error", e)])),
            Self::Codec(e) => f.write_str(&trf("error.image", &[("error", e)])),
            Self::Encode(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ImageError {}

impl From<std::io::Error> for ImageError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<image::ImageError> for ImageError {
    fn from(e: image::ImageError) -> Self {
        Self::Codec(e)
    }
}

/// Cap on a frame decoded through ffmpeg (formats the image crate lacks, e.g. HEIC).
const MAX_FFMPEG_DECODE_BYTES: u64 = 512 << 20;

fn open(path: &Path, ffmpeg: Option<&Path>) -> Result<DynamicImage, ImageError> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    match (reader.decode(), ffmpeg) {
        (Err(image::ImageError::Unsupported(_)), Some(ffmpeg)) => decode_with_ffmpeg(ffmpeg, path),
        (result, _) => Ok(result?),
    }
}

/// Lets ffmpeg decode the first frame to PNG in memory.
fn decode_with_ffmpeg(ffmpeg: &Path, path: &Path) -> Result<DynamicImage, ImageError> {
    let mut url = std::ffi::OsString::from("file:");
    url.push(path.as_os_str());
    let mut child = crate::process::command(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(url)
        .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "-"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(crate::process::KillOnDrop)?;
    let mut png = Vec::new();
    if let Some(stdout) = child.0.stdout.take() {
        use std::io::Read;
        stdout.take(MAX_FFMPEG_DECODE_BYTES).read_to_end(&mut png)?;
    }
    let _ = child.0.wait();
    Ok(image::load_from_memory_with_format(
        &png,
        image::ImageFormat::Png,
    )?)
}

/// Applies resize/rotation/grayscale options. Never upscales.
fn transform(img: DynamicImage, opts: &Options, hard_cap: Option<u32>) -> DynamicImage {
    let user_cap = opts
        .image_max_side
        .map(|s| s.clamp(options::MIN_IMAGE_SIDE, options::MAX_IMAGE_SIDE));
    let cap = match (user_cap, hard_cap) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let img = match cap {
        Some(side) if img.width().max(img.height()) > side => {
            img.resize(side, side, FilterType::Lanczos3)
        }
        _ => img,
    };
    let img = match opts.image_rotation {
        Some(Rotation::Clockwise) => img.rotate90(),
        Some(Rotation::CounterClockwise) => img.rotate270(),
        Some(Rotation::UpsideDown) => img.rotate180(),
        Some(Rotation::FlipHorizontal) => img.fliph(),
        Some(Rotation::FlipVertical) => img.flipv(),
        None => img,
    };
    if opts.grayscale { img.grayscale() } else { img }
}

/// Composites transparency onto white, since JPEG has no alpha channel.
fn flatten(img: &DynamicImage) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let blend =
            |c: u8| ((u16::from(c) * u16::from(a) + 255 * (255 - u16::from(a))) / 255) as u8;
        image::Rgb([blend(r), blend(g), blend(b)])
    })
}

fn jpeg_quality(opts: &Options) -> u8 {
    opts.quality.pick(92, 82, 65)
}

fn encode_jpeg(img: &DynamicImage, quality: u8, out: impl Write) -> Result<(), ImageError> {
    let rgb = flatten(img);
    JpegEncoder::new_with_quality(out, quality).encode_image(&rgb)?;
    Ok(())
}

/// Converts one image, writing atomically (temp file + rename) next to `output`.
pub fn convert(
    input: &Path,
    output: &Path,
    format: ImageFormat,
    opts: &Options,
    ffmpeg: Option<&Path>,
) -> Result<(), ImageError> {
    let hard_cap = (format == ImageFormat::Ico).then_some(ICO_MAX_SIDE);
    let img = watermark(
        transform(open(input, ffmpeg)?, opts, hard_cap),
        opts,
        ffmpeg,
    )?;
    write_image(&img, output, format, opts, ffmpeg)
}

/// Encodes `img` to `output` atomically, natively or through ffmpeg.
fn write_image(
    img: &DynamicImage,
    output: &Path,
    format: ImageFormat,
    opts: &Options,
    ffmpeg: Option<&Path>,
) -> Result<(), ImageError> {
    let dir = output.parent().unwrap_or(Path::new("."));
    let Some(codec) = format.codec() else {
        return encode_with_ffmpeg(img, output, format, ffmpeg);
    };
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut out = BufWriter::new(tmp.as_file_mut());
        match format {
            ImageFormat::Jpeg => encode_jpeg(img, jpeg_quality(opts), &mut out)?,
            // PNG and TIFF keep 16-bit depth; the rest want 8-bit RGBA.
            ImageFormat::Png | ImageFormat::Tiff => img.write_to(&mut out, codec)?,
            _ => DynamicImage::ImageRgba8(img.to_rgba8()).write_to(&mut out, codec)?,
        }
        out.flush()?;
    }
    tmp.persist(output).map_err(|e| ImageError::Io(e.error))?;
    Ok(())
}

/// Hands a lossless PNG to ffmpeg for formats with no native encoder (AVIF, JPEG XL, PCX).
fn encode_with_ffmpeg(
    img: &DynamicImage,
    output: &Path,
    format: ImageFormat,
    ffmpeg: Option<&Path>,
) -> Result<(), ImageError> {
    let ffmpeg = ffmpeg.ok_or_else(|| {
        ImageError::Encode(crate::i18n::trf(
            "error.missing_program",
            &[("program", &"ffmpeg")],
        ))
    })?;
    let flags = format.ffmpeg_encoder().unwrap_or_default();
    let dir = output.parent().unwrap_or(Path::new("."));
    let staging = tempfile::Builder::new().suffix(".png").tempfile_in(dir)?;
    DynamicImage::ImageRgba8(img.to_rgba8())
        .save_with_format(staging.path(), image::ImageFormat::Png)?;
    let encoded = tempfile::Builder::new()
        .suffix(&format!(".{}", format.ext()))
        .tempfile_in(dir)?;
    let mut args: Vec<std::ffi::OsString> =
        ["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-i"]
            .map(Into::into)
            .to_vec();
    args.push(crate::plan::input_url(staging.path(), None));
    args.extend(flags.iter().map(Into::into));
    args.extend(["-frames:v".into(), "1".into(), "-update".into(), "1".into()]);
    args.push(crate::plan::input_url(encoded.path(), None));
    let never = std::sync::atomic::AtomicBool::new(false);
    crate::process::run(
        ffmpeg,
        &args,
        crate::process::StopMode::Kill,
        &never,
        |_| {},
    )
    .map_err(|e| ImageError::Encode(e.to_string()))?;
    encoded
        .persist(output)
        .map_err(|e| ImageError::Io(e.error))?;
    Ok(())
}

/// Overlays the batch's watermark image (if any) in its chosen corner.
fn watermark(
    mut img: DynamicImage,
    opts: &Options,
    ffmpeg: Option<&Path>,
) -> Result<DynamicImage, ImageError> {
    let Some(path) = &opts.edit.watermark else {
        return Ok(img);
    };
    let mut mark = open(path, ffmpeg)?;
    // Keep the mark at most a quarter of the picture's width.
    let max_width = (img.width() / 4).max(1);
    if mark.width() > max_width {
        mark = mark.resize(max_width, u32::MAX, FilterType::Lanczos3);
    }
    let (x, y) = corner_position(
        img.width(),
        img.height(),
        mark.width(),
        mark.height(),
        opts.edit.watermark_corner.unwrap_or(Corner::BottomRight),
    );
    let mut canvas = img.to_rgba8();
    image::imageops::overlay(&mut canvas, &mark.to_rgba8(), x, y);
    img = DynamicImage::ImageRgba8(canvas);
    Ok(img)
}

const WATERMARK_MARGIN: i64 = 16;

fn corner_position(
    width: u32,
    height: u32,
    mark_w: u32,
    mark_h: u32,
    corner: Corner,
) -> (i64, i64) {
    let (w, h, mw, mh) = (
        i64::from(width),
        i64::from(height),
        i64::from(mark_w),
        i64::from(mark_h),
    );
    let m = WATERMARK_MARGIN
        .min((w - mw).max(0) / 2)
        .min((h - mh).max(0) / 2);
    match corner {
        Corner::TopLeft => (m, m),
        Corner::TopRight => (w - mw - m, m),
        Corner::BottomLeft => (m, h - mh - m),
        Corner::BottomRight => (w - mw - m, h - mh - m),
        Corner::Center => ((w - mw) / 2, (h - mh) / 2),
    }
}

/// Stitches pictures into one, top-to-bottom or left-to-right, sized to match.
pub fn join(
    inputs: &[impl AsRef<Path>],
    output: &Path,
    layout: JoinLayout,
    opts: &Options,
    ffmpeg: Option<&Path>,
    mut on_progress: impl FnMut(f32),
) -> Result<(), ImageError> {
    let mut pictures = Vec::with_capacity(inputs.len());
    for (i, path) in inputs.iter().enumerate() {
        pictures.push(transform(open(path.as_ref(), ffmpeg)?, opts, None));
        on_progress((i + 1) as f32 / (inputs.len() + 1) as f32);
    }
    let canvas = stitch(&pictures, layout);
    let img = watermark(DynamicImage::ImageRgba8(canvas), opts, ffmpeg)?;
    write_image(&img, output, ImageFormat::Png, opts, ffmpeg)
}

/// Largest side length the stitched image may reach.
const MAX_STITCH_SIDE: u32 = 32_768;

fn stitch(pictures: &[DynamicImage], layout: JoinLayout) -> image::RgbaImage {
    // Match the smallest width (vertical) or height (horizontal) so nothing is upscaled.
    let across = |p: &DynamicImage| {
        if layout == JoinLayout::Vertical {
            p.width()
        } else {
            p.height()
        }
    };
    let target = pictures.iter().map(across).min().unwrap_or(1).max(1);
    let scaled: Vec<image::RgbaImage> = pictures
        .iter()
        .map(|p| {
            let (w, h) = (p.width().max(1), p.height().max(1));
            let (nw, nh) = match layout {
                JoinLayout::Vertical => (
                    target,
                    (u64::from(h) * u64::from(target) / u64::from(w)).max(1) as u32,
                ),
                JoinLayout::Horizontal => (
                    (u64::from(w) * u64::from(target) / u64::from(h)).max(1) as u32,
                    target,
                ),
            };
            p.resize_exact(nw, nh, FilterType::Lanczos3).to_rgba8()
        })
        .collect();
    let along: u32 = scaled
        .iter()
        .map(|p| {
            if layout == JoinLayout::Vertical {
                p.height()
            } else {
                p.width()
            }
        })
        .fold(0u32, u32::saturating_add)
        .min(MAX_STITCH_SIDE);
    let (cw, ch) = if layout == JoinLayout::Vertical {
        (target, along)
    } else {
        (along, target)
    };
    let mut canvas = image::RgbaImage::new(cw, ch);
    let mut offset = 0i64;
    for p in &scaled {
        let (x, y) = if layout == JoinLayout::Vertical {
            (0, offset)
        } else {
            (offset, 0)
        };
        image::imageops::overlay(&mut canvas, p, x, y);
        offset += i64::from(if layout == JoinLayout::Vertical {
            p.height()
        } else {
            p.width()
        });
    }
    canvas
}

/// One JPEG-encoded page ready for embedding.
struct Page {
    width: u32,
    height: u32,
    jpeg: Vec<u8>,
}

/// Writes all `inputs` as pages of a single PDF, in order, reporting progress.
pub fn images_to_pdf(
    inputs: &[impl AsRef<Path>],
    output: &Path,
    opts: &Options,
    ffmpeg: Option<&Path>,
    mut on_progress: impl FnMut(f32),
) -> Result<(), ImageError> {
    let mut pages = Vec::with_capacity(inputs.len());
    for (i, path) in inputs.iter().enumerate() {
        let img = watermark(
            transform(open(path.as_ref(), ffmpeg)?, opts, None),
            opts,
            ffmpeg,
        )?;
        let mut jpeg = Vec::new();
        encode_jpeg(&img, jpeg_quality(opts), &mut jpeg)?;
        pages.push(Page {
            width: img.width(),
            height: img.height(),
            jpeg,
        });
        on_progress((i + 1) as f32 / inputs.len() as f32);
    }
    let dir = output.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut out = BufWriter::new(tmp.as_file_mut());
        write_pdf(&pages, &mut out)?;
        out.flush()?;
    }
    tmp.persist(output).map_err(|e| ImageError::Io(e.error))?;
    Ok(())
}

/// Counts bytes written so object offsets for the xref table are exact.
struct Counting<W> {
    inner: W,
    pos: usize,
}

impl<W: Write> Counting<W> {
    fn put(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.inner.write_all(bytes)?;
        self.pos += bytes.len();
        Ok(())
    }
}

/// Minimal PDF 1.4 writer: catalog, page tree, and per page an image XObject
/// (DCTDecode, i.e. the JPEG bytes verbatim) plus a content stream drawing it.
fn write_pdf(pages: &[Page], out: impl Write) -> std::io::Result<()> {
    // Object numbering: 1 catalog, 2 page tree, then 3 objects per page.
    let page_obj = |i: usize| 3 + 3 * i;
    let mut out = Counting { inner: out, pos: 0 };
    let mut offsets = Vec::with_capacity(2 + 3 * pages.len());

    out.put(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n")?;
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", page_obj(i)))
        .collect();
    offsets.push(out.pos);
    out.put(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n")?;
    offsets.push(out.pos);
    out.put(
        format!(
            "2 0 obj\n<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n",
            kids.join(" "),
            pages.len()
        )
        .as_bytes(),
    )?;

    for (i, page) in pages.iter().enumerate() {
        let (p, img, content) = (page_obj(i), page_obj(i) + 1, page_obj(i) + 2);
        let w = f64::from(page.width) * POINTS_PER_PIXEL;
        let h = f64::from(page.height) * POINTS_PER_PIXEL;
        let draw = format!("q {w:.2} 0 0 {h:.2} 0 0 cm /Im0 Do Q");

        offsets.push(out.pos);
        out.put(
            format!(
                "{p} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w:.2} {h:.2}] \
             /Resources << /XObject << /Im0 {img} 0 R >> >> /Contents {content} 0 R >>\nendobj\n"
            )
            .as_bytes(),
        )?;

        offsets.push(out.pos);
        out.put(
            format!(
                "{img} 0 obj\n<< /Type /XObject /Subtype /Image /Width {} /Height {} \
             /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
                page.width,
                page.height,
                page.jpeg.len()
            )
            .as_bytes(),
        )?;
        out.put(&page.jpeg)?;
        out.put(b"\nendstream\nendobj\n")?;

        offsets.push(out.pos);
        out.put(
            format!(
                "{content} 0 obj\n<< /Length {} >>\nstream\n{draw}\nendstream\nendobj\n",
                draw.len()
            )
            .as_bytes(),
        )?;
    }

    let xref_at = out.pos;
    let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for offset in &offsets {
        xref.push_str(&format!("{offset:010} 00000 n \n"));
    }
    xref.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        offsets.len() + 1
    ));
    out.put(xref.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn sample(dir: &Path, name: &str, w: u32, h: u32) -> std::path::PathBuf {
        let path = dir.join(name);
        RgbaImage::from_pixel(w, h, Rgba([200, 30, 30, 128]))
            .save(&path)
            .expect("write sample");
        path
    }

    #[test]
    fn converts_png_to_every_format() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = sample(dir.path(), "in.png", 40, 30);
        for format in ImageFormat::ALL.into_iter().filter(|f| f.codec().is_some()) {
            let out = dir.path().join(format!("out.{}", format.ext()));
            convert(&src, &out, format, &Options::default(), None)
                .unwrap_or_else(|e| panic!("{format:?}: {e}"));
            let decoded = image::open(&out).unwrap_or_else(|e| panic!("{format:?} reopen: {e}"));
            assert_eq!((decoded.width(), decoded.height()), (40, 30), "{format:?}");
        }
    }

    #[test]
    fn resize_caps_long_side_without_upscaling() {
        let opts = Options {
            image_max_side: Some(20),
            ..Options::default()
        };
        let img = transform(DynamicImage::new_rgb8(80, 40), &opts, None);
        assert_eq!((img.width(), img.height()), (20, 10));
        let small = transform(DynamicImage::new_rgb8(10, 5), &opts, None);
        assert_eq!((small.width(), small.height()), (10, 5));
    }

    #[test]
    fn join_stacks_to_common_width() {
        let a = DynamicImage::new_rgb8(100, 50);
        let b = DynamicImage::new_rgb8(200, 100);
        let v = stitch(&[a.clone(), b.clone()], JoinLayout::Vertical);
        assert_eq!((v.width(), v.height()), (100, 100));
        let h = stitch(&[a, b], JoinLayout::Horizontal);
        assert_eq!((h.width(), h.height()), (200, 50));
    }

    #[test]
    fn watermark_positions_stay_inside() {
        assert_eq!(
            corner_position(200, 100, 20, 10, Corner::BottomRight),
            (164, 74)
        );
        assert_eq!(corner_position(200, 100, 20, 10, Corner::TopLeft), (16, 16));
        // A mark as large as the picture sits at the origin.
        assert_eq!(corner_position(50, 50, 50, 50, Corner::BottomRight), (0, 0));
    }

    #[test]
    fn rotation_swaps_dimensions() {
        let opts = Options {
            image_rotation: Some(Rotation::Clockwise),
            ..Options::default()
        };
        let img = transform(DynamicImage::new_rgb8(80, 40), &opts, None);
        assert_eq!((img.width(), img.height()), (40, 80));
    }

    #[test]
    fn ico_is_capped_at_256() {
        let img = transform(
            DynamicImage::new_rgb8(1000, 500),
            &Options::default(),
            Some(ICO_MAX_SIDE),
        );
        assert_eq!(img.width(), 256);
    }

    #[test]
    fn flatten_blends_onto_white() {
        let mut rgba = RgbaImage::new(1, 1);
        rgba.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        assert_eq!(
            flatten(&DynamicImage::ImageRgba8(rgba)).get_pixel(0, 0).0,
            [255, 255, 255]
        );
    }

    #[test]
    fn pdf_has_valid_xref_offsets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = sample(dir.path(), "a.png", 30, 20);
        let b = sample(dir.path(), "b.png", 10, 40);
        let out = dir.path().join("out.pdf");
        let mut last = 0.0;
        images_to_pdf(&[a, b], &out, &Options::default(), None, |p| last = p).expect("pdf");
        assert_eq!(last, 1.0);

        let bytes = std::fs::read(&out).expect("read pdf");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.4") && text.contains("/Count 2"));
        let xref_at: usize = text
            .rsplit("startxref\n")
            .next()
            .and_then(|t| t.lines().next())
            .and_then(|n| n.parse().ok())
            .expect("startxref");
        assert!(bytes[xref_at..].starts_with(b"xref"));
        // Every object offset must point at "<n> 0 obj".
        let table = String::from_utf8_lossy(&bytes[xref_at..]);
        for (n, line) in table.lines().skip(3).take(8).enumerate() {
            let offset: usize = line[..10].parse().expect("offset");
            let expected = format!("{} 0 obj", n + 1);
            assert!(
                bytes[offset..].starts_with(expected.as_bytes()),
                "object {}",
                n + 1
            );
        }
    }
}
