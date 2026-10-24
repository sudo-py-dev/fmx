//! Native (ffmpeg-free) image conversion and images-to-PDF.

use std::fmt;
use std::io::{BufWriter, Write};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageReader, RgbImage};

use crate::catalog::ImageFormat;
use crate::i18n::trf;
use crate::options::{self, Options};

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
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => f.write_str(&trf("error.io", &[("error", e)])),
            Self::Codec(e) => f.write_str(&trf("error.image", &[("error", e)])),
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

/// Whether the `image` crate can decode this file (judged by extension).
pub fn is_supported_image(path: &Path) -> bool {
    image::ImageFormat::from_path(path).is_ok_and(|f| f.reading_enabled())
}

fn open(path: &Path) -> Result<DynamicImage, ImageError> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    Ok(reader.decode()?)
}

/// Applies resize/grayscale options. Never upscales.
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
) -> Result<(), ImageError> {
    let hard_cap = (format == ImageFormat::Ico).then_some(ICO_MAX_SIDE);
    let img = transform(open(input)?, opts, hard_cap);
    let dir = output.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut out = BufWriter::new(tmp.as_file_mut());
        match format {
            ImageFormat::Jpeg => encode_jpeg(&img, jpeg_quality(opts), &mut out)?,
            // PNG and TIFF keep 16-bit depth; the rest want 8-bit RGBA.
            ImageFormat::Png | ImageFormat::Tiff => img.write_to(&mut out, format.codec())?,
            _ => DynamicImage::ImageRgba8(img.to_rgba8()).write_to(&mut out, format.codec())?,
        }
        out.flush()?;
    }
    tmp.persist(output).map_err(|e| ImageError::Io(e.error))?;
    Ok(())
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
    mut on_progress: impl FnMut(f32),
) -> Result<(), ImageError> {
    let mut pages = Vec::with_capacity(inputs.len());
    for (i, path) in inputs.iter().enumerate() {
        let img = transform(open(path.as_ref())?, opts, None);
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
        for format in ImageFormat::ALL {
            let out = dir.path().join(format!("out.{}", format.ext()));
            convert(&src, &out, format, &Options::default())
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
        images_to_pdf(&[a, b], &out, &Options::default(), |p| last = p).expect("pdf");
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
