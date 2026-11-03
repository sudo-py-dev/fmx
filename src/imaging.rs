//! Native (ffmpeg-free) image conversion and images-to-PDF.

use std::fmt;
use std::io::{BufWriter, Write};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::pnm::{PnmEncoder, PnmSubtype, SampleEncoding};
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
        stdout
            .take(MAX_FFMPEG_DECODE_BYTES + 1)
            .read_to_end(&mut png)?;
        if png.len() as u64 > MAX_FFMPEG_DECODE_BYTES {
            return Err(ImageError::Encode(
                "decoded image exceeds size limit".into(),
            ));
        }
    }
    let status = child.0.wait()?;
    if !status.success() {
        return Err(ImageError::Encode("ffmpeg image decoding failed".into()));
    }
    decode_png(&png, MAX_FFMPEG_DECODE_BYTES)
}

fn decode_png(png: &[u8], max_alloc: u64) -> Result<DynamicImage, ImageError> {
    let mut reader = ImageReader::with_format(std::io::Cursor::new(png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(max_alloc);
    limits.max_image_width = Some(32_768);
    limits.max_image_height = Some(32_768);
    reader.limits(limits);
    Ok(reader.decode()?)
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

/// libwebp's quality for lossy WebP.
fn webp_quality(opts: &Options) -> &'static str {
    opts.quality.pick("90", "80", "65")
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
    // The native WebP encoder is lossless only, which makes photos several
    // times larger than the JPEG they came from; libwebp encodes them lossy.
    if format == ImageFormat::Webp && ffmpeg.is_some() {
        let flags = [
            "-c:v",
            "libwebp",
            "-quality",
            webp_quality(opts),
            "-compression_level",
            "6",
        ];
        return encode_with_ffmpeg(img, output, format, &flags, ffmpeg);
    }
    let Some(codec) = format.codec() else {
        let flags = format.ffmpeg_encoder().unwrap_or_default();
        return encode_with_ffmpeg(img, output, format, flags, ffmpeg);
    };
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    {
        let mut out = BufWriter::new(tmp.as_file_mut());
        match format {
            ImageFormat::Ppm => {
                let rgb = flatten(img);
                PnmEncoder::new(&mut out)
                    .with_subtype(PnmSubtype::Pixmap(SampleEncoding::Binary))
                    .encode(
                        rgb.as_raw().as_slice(),
                        img.width(),
                        img.height(),
                        image::ExtendedColorType::Rgb8,
                    )?;
            }
            ImageFormat::Pgm => {
                let gray = DynamicImage::ImageRgb8(flatten(img)).to_luma8();
                PnmEncoder::new(&mut out)
                    .with_subtype(PnmSubtype::Graymap(SampleEncoding::Binary))
                    .encode(
                        gray.as_raw().as_slice(),
                        img.width(),
                        img.height(),
                        image::ExtendedColorType::L8,
                    )?;
            }
            ImageFormat::Pam => {
                let rgba = img.to_rgba8();
                PnmEncoder::new(&mut out)
                    .with_subtype(PnmSubtype::ArbitraryMap)
                    .encode(
                        rgba.as_raw().as_slice(),
                        img.width(),
                        img.height(),
                        image::ExtendedColorType::Rgba8,
                    )?;
            }
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

/// Hands a lossless PNG to ffmpeg, which encodes it with `flags`: for formats
/// with no native encoder (AVIF, JPEG XL, PCX) and for lossy WebP.
fn encode_with_ffmpeg(
    img: &DynamicImage,
    output: &Path,
    format: ImageFormat,
    flags: &[&str],
    ffmpeg: Option<&Path>,
) -> Result<(), ImageError> {
    let ffmpeg = ffmpeg.ok_or_else(|| {
        ImageError::Encode(crate::i18n::trf(
            "error.missing_program",
            &[("program", &"ffmpeg")],
        ))
    })?;
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
    let canvas = if matches!(layout, JoinLayout::Vertical | JoinLayout::Horizontal) {
        stitch(&pictures, layout)
    } else {
        let edits: Vec<_> = inputs
            .iter()
            .map(|path| {
                opts.collage_photos
                    .get(path.as_ref())
                    .copied()
                    .unwrap_or_default()
            })
            .collect();
        collage_filtered(&pictures, layout, opts, FilterType::Lanczos3, &edits)
    };
    let img = watermark(DynamicImage::ImageRgba8(canvas), opts, ffmpeg)?;
    write_image(&img, output, ImageFormat::Png, opts, ffmpeg)
}

/// Decode off the UI thread and retain a bounded source for interactive edits.
pub fn collage_photo(path: &Path, ffmpeg: Option<&Path>) -> Result<DynamicImage, ImageError> {
    // Decode once; retain only the small, unadjusted source while editing.
    Ok(open(path, ffmpeg)?.thumbnail(640, 640))
}

#[cfg(test)]
pub fn collage_preview(
    pictures: &[&DynamicImage],
    paths: &[std::path::PathBuf],
    opts: &Options,
    ffmpeg: Option<&Path>,
) -> Result<image::RgbaImage, ImageError> {
    collage_preview_sized(pictures, paths, opts, ffmpeg, 640)
}

pub fn collage_preview_sized(
    pictures: &[&DynamicImage],
    paths: &[std::path::PathBuf],
    opts: &Options,
    ffmpeg: Option<&Path>,
    side: u32,
) -> Result<image::RgbaImage, ImageError> {
    let side = side.clamp(256, 1024);
    let mut preview = opts.clone();
    preview.collage_gap = (opts.collage_gap as f32 * side as f32
        / opts.collage_size.clamp(256, 4096) as f32)
        .round() as u32;
    preview.collage_size = side;
    let pictures: Vec<_> = pictures
        .iter()
        .map(|picture| {
            if opts.image_rotation.is_none() && !opts.grayscale && opts.image_max_side.is_none() {
                std::borrow::Cow::Borrowed(*picture)
            } else {
                std::borrow::Cow::Owned(transform((*picture).clone(), opts, None))
            }
        })
        .collect();
    let edits: Vec<_> = paths
        .iter()
        .map(|path| opts.collage_photos.get(path).copied().unwrap_or_default())
        .collect();
    let canvas = collage_filtered(
        &pictures,
        opts.join_layout,
        &preview,
        FilterType::Triangle,
        &edits,
    );
    Ok(watermark(DynamicImage::ImageRgba8(canvas), &preview, ffmpeg)?.to_rgba8())
}

/// A bounded square canvas keeps template exports predictable and inexpensive.
#[cfg(test)]
fn collage(
    pictures: &[impl std::borrow::Borrow<DynamicImage>],
    layout: JoinLayout,
    opts: &Options,
) -> image::RgbaImage {
    collage_filtered(pictures, layout, opts, FilterType::Lanczos3, &[])
}

fn collage_filtered(
    pictures: &[impl std::borrow::Borrow<DynamicImage>],
    layout: JoinLayout,
    opts: &Options,
    filter: FilterType,
    edits: &[options::CollagePhotoEdit],
) -> image::RgbaImage {
    let size = opts.collage_size.clamp(256, 4096);
    let gap = opts.collage_gap.min(size / 16);
    let mut canvas = image::RgbaImage::from_pixel(size, size, image::Rgba([255, 255, 255, 255]));
    for (i, (picture, [x, y, w, h])) in pictures
        .iter()
        .zip(opts.collage_slots_for(layout, pictures.len()))
        .enumerate()
    {
        let edit = edits.get(i).copied().unwrap_or_default();
        let picture = picture.borrow();
        let rotated;
        let picture = match edit.turns % 4 {
            1 => {
                rotated = picture.rotate90();
                &rotated
            }
            2 => {
                rotated = picture.rotate180();
                &rotated
            }
            3 => {
                rotated = picture.rotate270();
                &rotated
            }
            _ => picture,
        };
        let left = (x * size as f32).round() as u32 + gap;
        let top = (y * size as f32).round() as u32 + gap;
        let right = ((x + w) * size as f32).round() as u32;
        let bottom = ((y + h) * size as f32).round() as u32;
        let width = right.saturating_sub(left + gap).max(1);
        let height = bottom.saturating_sub(top + gap).max(1);
        // Crop the source before resizing: zoom never allocates an oversized bitmap.
        let sx = width as f32 / picture.width() as f32;
        let sy = height as f32 / picture.height() as f32;
        let scale = if opts.collage_crop {
            sx.max(sy)
        } else {
            sx.min(sy)
        } * edit.zoom.clamp(1.0, 4.0);
        let visible_w = (picture.width() as f32 * scale)
            .round()
            .clamp(1.0, width as f32) as u32;
        let visible_h = (picture.height() as f32 * scale)
            .round()
            .clamp(1.0, height as f32) as u32;
        let crop_w = (visible_w as f32 / scale)
            .round()
            .clamp(1.0, picture.width() as f32) as u32;
        let crop_h = (visible_h as f32 / scale)
            .round()
            .clamp(1.0, picture.height() as f32) as u32;
        let x = edit.x.clamp(0.0, 1.0);
        let y = edit.y.clamp(0.0, 1.0);
        let image = picture
            .crop_imm(
                ((picture.width() - crop_w) as f32 * x).round() as u32,
                ((picture.height() - crop_h) as f32 * y).round() as u32,
                crop_w,
                crop_h,
            )
            .resize_exact(visible_w, visible_h, filter);
        let offset_x = ((width - visible_w) as f32 * x).round() as u32;
        let offset_y = ((height - visible_h) as f32 * y).round() as u32;
        let mut tile =
            image::RgbaImage::from_pixel(width, height, image::Rgba([255, 255, 255, 255]));
        if edit.angle.abs() > 0.01 {
            let radians = edit.angle.to_radians();
            let (sin, cos) = radians.sin_cos();
            let iw = image.width() as f32;
            let ih = image.height() as f32;
            let (w, h) = (width as f32, height as f32);
            let scale = if opts.collage_crop {
                ((w * cos.abs() + h * sin.abs()) / iw).max((w * sin.abs() + h * cos.abs()) / ih)
            } else {
                (w / (iw * cos.abs() + ih * sin.abs())).min(h / (iw * sin.abs() + ih * cos.abs()))
            };
            let source = image.to_rgba8();
            for (px, py, pixel) in tile.enumerate_pixels_mut() {
                let dx = (px as f32 + 0.5 - w / 2.0) / scale;
                let dy = (py as f32 + 0.5 - h / 2.0) / scale;
                let sx = cos * dx + sin * dy + iw / 2.0 - 0.5;
                let sy = -sin * dx + cos * dy + ih / 2.0 - 0.5;
                if sx >= 0.0 && sy >= 0.0 && sx < iw && sy < ih {
                    let x0 = sx.floor() as u32;
                    let y0 = sy.floor() as u32;
                    let x1 = (x0 + 1).min(source.width() - 1);
                    let y1 = (y0 + 1).min(source.height() - 1);
                    let (fx, fy) = (sx.fract(), sy.fract());
                    let samples = [
                        (x0, y0, (1.0 - fx) * (1.0 - fy)),
                        (x1, y0, fx * (1.0 - fy)),
                        (x0, y1, (1.0 - fx) * fy),
                        (x1, y1, fx * fy),
                    ];
                    // Premultiplied blending keeps transparent edges free of dark halos.
                    let mut rgba = [0.0_f32; 4];
                    for (sx, sy, weight) in samples {
                        let color = source.get_pixel(sx, sy).0;
                        let alpha = color[3] as f32 / 255.0;
                        for channel in 0..3 {
                            rgba[channel] += color[channel] as f32 * alpha * weight;
                        }
                        rgba[3] += alpha * weight;
                    }
                    for channel in 0..3 {
                        pixel[channel] = (rgba[channel] + 255.0 * (1.0 - rgba[3])).round() as u8;
                    }
                }
            }
        } else {
            image::imageops::overlay(&mut tile, &image, i64::from(offset_x), i64::from(offset_y));
        }
        if matches!(
            opts.collage_frame,
            options::CollageFrame::Rounded | options::CollageFrame::Circle
        ) {
            let half_w = width as f32 / 2.0;
            let half_h = height as f32 / 2.0;
            let radius = if opts.collage_frame == options::CollageFrame::Circle {
                half_w.min(half_h)
            } else {
                width.min(height) as f32 * 0.15
            };
            for (px, py, pixel) in tile.enumerate_pixels_mut() {
                let dx = ((px as f32 + 0.5 - half_w).abs() - (half_w - radius)).max(0.0);
                let dy = ((py as f32 + 0.5 - half_h).abs() - (half_h - radius)).max(0.0);
                let coverage = (radius + 0.5 - dx.hypot(dy)).clamp(0.0, 1.0);
                for channel in 0..3 {
                    pixel[channel] =
                        (pixel[channel] as f32 * coverage + 255.0 * (1.0 - coverage)).round() as u8;
                }
                pixel[3] = (pixel[3] as f32 * coverage + 255.0 * (1.0 - coverage)).round() as u8;
            }
        }
        image::imageops::overlay(&mut canvas, &tile, i64::from(left), i64::from(top));
    }
    canvas
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
                _ => (
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

    #[test]
    fn interactive_preview_is_small_and_keeps_composition() {
        let photo =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(400, 400, Rgba([255, 0, 0, 255])));
        let opts = Options {
            collage_gap: 0,
            join_layout: JoinLayout::Grid2,
            ..Options::default()
        };
        let draft = collage_preview_sized(&[&photo], &[], &opts, None, 320).unwrap();
        let settled = collage_preview_sized(&[&photo], &[], &opts, None, 640).unwrap();
        assert_eq!(draft.dimensions(), (320, 320));
        assert_eq!(settled.dimensions(), (640, 640));
        assert_eq!(draft.get_pixel(160, 160), settled.get_pixel(320, 320));
        assert_eq!(draft.get_pixel(0, 0), settled.get_pixel(0, 0));
    }

    #[test]
    fn frame_shapes_and_fine_rotation_match_preview_and_export() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("halves.png");
        RgbaImage::from_fn(800, 800, |x, _| {
            if x < 400 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        })
        .save(&source)
        .unwrap();
        let photo = collage_photo(&source, None).unwrap();
        for frame in options::CollageFrame::ALL {
            let mut opts = Options {
                join_layout: JoinLayout::Grid2,
                collage_gap: 0,
                collage_size: 1280,
                collage_frame: frame,
                ..Options::default()
            };
            opts.collage_photos.insert(
                source.clone(),
                options::CollagePhotoEdit {
                    angle: 35.0,
                    ..Default::default()
                },
            );
            let [x, y, w, h] = opts.collage_slots(1)[0];
            if matches!(
                frame,
                options::CollageFrame::Square | options::CollageFrame::Circle
            ) {
                assert_eq!(w, h);
            }
            let preview =
                collage_preview(&[&photo], std::slice::from_ref(&source), &opts, None).unwrap();
            let output = dir.path().join(format!("{frame:?}.png"));
            join(
                std::slice::from_ref(&source),
                &output,
                opts.join_layout,
                &opts,
                None,
                |_| {},
            )
            .unwrap();
            let exported = image::open(output).unwrap().to_rgba8();
            for (fraction, expected) in [
                (0.15, Rgba([255, 0, 0, 255])),
                (0.85, Rgba([0, 0, 255, 255])),
            ] {
                let px = ((x + w * 0.5) * 640.0) as u32;
                let py = ((y + h * fraction) * 640.0) as u32;
                assert_eq!(preview.get_pixel(px, py), &expected, "{frame:?}");
                assert_eq!(exported.get_pixel(px * 2, py * 2), &expected, "{frame:?}");
            }
            if matches!(
                frame,
                options::CollageFrame::Rounded | options::CollageFrame::Circle
            ) {
                assert_eq!(
                    preview.get_pixel((x * 640.0) as u32, (y * 640.0) as u32),
                    &Rgba([255; 4])
                );
                assert_eq!(
                    exported.get_pixel((x * 1280.0) as u32, (y * 1280.0) as u32),
                    &Rgba([255; 4])
                );
            }
        }
    }

    #[test]
    fn photo_framing_follows_source_when_reordered_and_exports() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("strip.png");
        let other = dir.path().join("green.png");
        RgbaImage::from_fn(800, 400, |x, _| {
            if x < 400 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        })
        .save(&source)
        .unwrap();
        RgbaImage::from_pixel(400, 400, Rgba([0, 255, 0, 255]))
            .save(&other)
            .unwrap();
        let mut opts = Options {
            join_layout: JoinLayout::Grid2,
            collage_gap: 0,
            collage_size: 1280,
            ..Options::default()
        };
        opts.collage_photos.insert(
            source.clone(),
            options::CollagePhotoEdit {
                zoom: 2.0,
                x: 1.0,
                y: 0.5,
                turns: 1,
                angle: 0.0,
            },
        );
        let photos = [
            collage_photo(&other, None).unwrap(),
            collage_photo(&source, None).unwrap(),
        ];
        let paths = [other, source];
        let preview = collage_preview(&[&photos[0], &photos[1]], &paths, &opts, None).unwrap();
        let output = dir.path().join("export.png");
        join(&paths, &output, opts.join_layout, &opts, None, |_| {}).unwrap();
        let export = image::open(output).unwrap().to_rgba8();
        for (x, y) in [(160, 100), (400, 100), (400, 540)] {
            assert_eq!(preview.get_pixel(x, y), export.get_pixel(x * 2, y * 2));
        }
        assert_eq!(export.get_pixel(320, 200), &Rgba([0, 255, 0, 255]));
        // The rotated second image has red above blue, with no edit on the green one.
        assert_eq!(export.get_pixel(800, 200), &Rgba([255, 0, 0, 255]));
        assert_eq!(export.get_pixel(800, 1080), &Rgba([0, 0, 255, 255]));
        let restored: Options =
            serde_json::from_str(&serde_json::to_string(&opts).unwrap()).unwrap();
        assert_eq!(restored.collage_photos, opts.collage_photos);
    }

    #[test]
    fn small_editing_sources_export_at_requested_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.png");
        RgbaImage::from_fn(1600, 800, |x, _| {
            if x < 800 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        })
        .save(&source)
        .unwrap();
        let photo = collage_photo(&source, None).unwrap();
        assert_eq!((photo.width(), photo.height()), (640, 320));
        let opts = Options {
            join_layout: JoinLayout::Grid2,
            collage_size: 1024,
            collage_gap: 0,
            image_rotation: Some(Rotation::FlipHorizontal),
            ..Options::default()
        };
        let preview =
            collage_preview(&[&photo], std::slice::from_ref(&source), &opts, None).unwrap();
        assert_eq!(preview.dimensions(), (640, 640));
        assert_eq!(preview.get_pixel(260, 320), &Rgba([0, 0, 255, 255]));
        let output = dir.path().join("collage.png");
        join(&[source], &output, opts.join_layout, &opts, None, |_| {}).unwrap();
        let exported = image::open(output).unwrap().to_rgba8();
        assert_eq!(exported.dimensions(), (1024, 1024));
        assert_eq!(exported.get_pixel(416, 512), preview.get_pixel(260, 320));
    }

    #[test]
    fn collage_grid_places_all_four_photos_with_white_gutters() {
        let colors = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [0, 0, 0, 255],
        ];
        let pictures: Vec<_> = colors
            .iter()
            .map(|c| DynamicImage::ImageRgba8(RgbaImage::from_pixel(30, 60, Rgba(*c))))
            .collect();
        let opts = Options {
            collage_size: 256,
            collage_gap: 8,
            ..Options::default()
        };
        let canvas = collage(&pictures, JoinLayout::Grid2, &opts);
        assert_eq!(canvas.dimensions(), (256, 256));
        for (point, color) in [(64, 64), (192, 64), (64, 192), (192, 192)]
            .iter()
            .zip(colors)
        {
            assert_eq!(canvas.get_pixel(point.0, point.1).0, color);
        }
        assert_eq!(canvas.get_pixel(128, 128).0, [255; 4]);
    }

    #[test]
    fn preview_matches_export_photo_order_and_gutters() {
        let red = DynamicImage::ImageRgba8(RgbaImage::from_pixel(40, 80, Rgba([255, 0, 0, 255])));
        let blue = DynamicImage::ImageRgba8(RgbaImage::from_pixel(80, 40, Rgba([0, 0, 255, 255])));
        let opts = Options {
            collage_size: 1280,
            collage_gap: 20,
            ..Options::default()
        };
        let preview = collage_preview(&[&blue, &red], &[], &opts, None).unwrap();
        let exported = collage(&[&blue, &red], opts.join_layout, &opts);
        assert_eq!(preview.dimensions(), (640, 640));
        for (x, y) in [(0, 0), (160, 320), (480, 320), (320, 320)] {
            assert_eq!(preview.get_pixel(x, y), exported.get_pixel(x * 2, y * 2));
        }
    }

    #[test]
    fn collage_templates_keep_every_slot_inside_canvas() {
        for layout in [JoinLayout::Grid2, JoinLayout::Grid3, JoinLayout::Featured] {
            for count in 0..12 {
                let slots = layout.slots(count);
                assert_eq!(slots.len(), count);
                for [x, y, w, h] in slots {
                    assert!(x >= 0.0 && y >= 0.0 && w > 0.0 && h > 0.0);
                    assert!(x + w <= 1.0001 && y + h <= 1.0001);
                }
            }
        }
    }

    fn sample(dir: &Path, name: &str, w: u32, h: u32) -> std::path::PathBuf {
        let path = dir.join(name);
        RgbaImage::from_pixel(w, h, Rgba([200, 30, 30, 128]))
            .save(&path)
            .expect("write sample");
        path
    }

    #[test]
    fn portable_images_use_correct_subtypes_and_preserve_pam_alpha() {
        let dir = tempfile::tempdir().unwrap();
        let src = sample(dir.path(), "in.png", 4, 3);
        for (format, magic) in [
            (ImageFormat::Ppm, b"P6"),
            (ImageFormat::Pgm, b"P5"),
            (ImageFormat::Pam, b"P7"),
        ] {
            let out = dir.path().join(format!("out.{}", format.ext()));
            convert(&src, &out, format, &Options::default(), None).unwrap();
            assert!(std::fs::read(&out).unwrap().starts_with(magic));
            let decoded = image::open(&out).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (4, 3));
            if format == ImageFormat::Pam {
                assert_eq!(decoded.to_rgba8().get_pixel(0, 0).0, [200, 30, 30, 128]);
            } else {
                assert!(!decoded.color().has_alpha());
            }
        }
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

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn fallback_png_rejects_excessive_dimensions() {
        let img = DynamicImage::new_luma8(32_769, 1);
        let mut encoded = std::io::Cursor::new(Vec::new());
        img.write_to(&mut encoded, image::ImageFormat::Png).unwrap();
        assert!(matches!(
            decode_png(encoded.get_ref(), MAX_FFMPEG_DECODE_BYTES),
            Err(ImageError::Codec(image::ImageError::Limits(_)))
        ));
    }

    #[test]
    fn fallback_png_obeys_decoded_memory_limit() {
        let img = DynamicImage::new_rgba8(128, 128);
        let mut encoded = std::io::Cursor::new(Vec::new());
        img.write_to(&mut encoded, image::ImageFormat::Png).unwrap();
        assert!(encoded.get_ref().len() < 4096);
        assert!(matches!(
            decode_png(encoded.get_ref(), 4096),
            Err(ImageError::Codec(image::ImageError::Limits(_)))
        ));
        assert!(decode_png(encoded.get_ref(), MAX_DECODE_BYTES).is_ok());
    }
}

/// Create a PNG from decoded pixels only: no EXIF/GPS, XMP, comments or source
/// profiles are passed to the encoder. Apply EXIF orientation before discarding it.
/// Only still JPEG, PNG, WebP and BMP files are accepted; no external fallback.
pub fn remove_metadata(input: &Path, output: &Path) -> Result<(), ImageError> {
    use image::ImageDecoder;
    use std::io::BufReader;
    let unsupported = || ImageError::Encode(crate::i18n::tr("photo.static_only").into());
    let metadata = std::fs::metadata(input)?;
    if !metadata.is_file() || metadata.len() > 128 * 1024 * 1024 {
        return Err(unsupported());
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(512 * 1024 * 1024);
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    let mut reader = ImageReader::open(input)?.with_guessed_format()?;
    let format = reader.format().ok_or_else(unsupported)?;
    match format {
        image::ImageFormat::Png => {
            let decoder = image::codecs::png::PngDecoder::with_limits(
                BufReader::new(std::fs::File::open(input)?),
                limits.clone(),
            )?;
            if decoder.is_apng()? {
                return Err(unsupported());
            }
        }
        image::ImageFormat::WebP => {
            let decoder =
                image::codecs::webp::WebPDecoder::new(BufReader::new(std::fs::File::open(input)?))?;
            if decoder.has_animation() {
                return Err(unsupported());
            }
        }
        image::ImageFormat::Jpeg | image::ImageFormat::Bmp => {}
        _ => return Err(unsupported()),
    }
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    write_image(&image, output, ImageFormat::Png, &Options::default(), None)
}

#[cfg(test)]
mod privacy_tests {
    use super::*;
    use image::{ImageDecoder, ImageEncoder};

    /// TIFF EXIF with orientation 6 and a real GPS latitude IFD.
    fn private_exif() -> Vec<u8> {
        let mut bytes = b"II\x2a\0\x08\0\0\0".to_vec();
        bytes.extend(2u16.to_le_bytes());
        for (tag, kind, count, value) in [(0x0112u16, 3u16, 1u32, 6u32), (0x8825, 4, 1, 38)] {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(kind.to_le_bytes());
            bytes.extend(count.to_le_bytes());
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        for (tag, kind, count, value) in [
            (1u16, 2u16, 2u32, u32::from_le_bytes(*b"N\0\0\0")),
            (2, 5, 3, 68),
        ] {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(kind.to_le_bytes());
            bytes.extend(count.to_le_bytes());
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(0u32.to_le_bytes());
        for n in [31u32, 42, 30] {
            bytes.extend(n.to_le_bytes());
            bytes.extend(1u32.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn privacy_copies_remove_gps_exif_preserve_pixels_and_apply_orientation() {
        let dir = tempfile::tempdir().unwrap();
        let pixels =
            image::RgbImage::from_fn(3, 2, |x, y| image::Rgb([x as u8 * 60, y as u8 * 90, 120]));
        for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Webp] {
            let source = dir.path().join(format!("private.{}", format.ext()));
            let out = std::fs::File::create(&source).unwrap();
            match format {
                ImageFormat::Jpeg => {
                    let mut encoder = JpegEncoder::new_with_quality(out, 92);
                    encoder.set_exif_metadata(private_exif()).unwrap();
                    encoder
                        .encode_image(&DynamicImage::ImageRgb8(pixels.clone()))
                        .unwrap();
                }
                ImageFormat::Png => {
                    let mut encoder = image::codecs::png::PngEncoder::new(out);
                    encoder.set_exif_metadata(private_exif()).unwrap();
                    encoder
                        .write_image(pixels.as_raw(), 3, 2, image::ExtendedColorType::Rgb8)
                        .unwrap();
                }
                ImageFormat::Webp => {
                    let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(out);
                    encoder.set_exif_metadata(private_exif()).unwrap();
                    encoder
                        .write_image(pixels.as_raw(), 3, 2, image::ExtendedColorType::Rgb8)
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let original = std::fs::read(&source).unwrap();
            let mut decoder = ImageReader::open(&source)
                .unwrap()
                .with_guessed_format()
                .unwrap()
                .into_decoder()
                .unwrap();
            assert!(decoder.exif_metadata().unwrap().is_some());
            let orientation = decoder.orientation().unwrap();
            assert_eq!(orientation, image::metadata::Orientation::Rotate90);
            let mut expected = open(&source, None).unwrap();
            expected.apply_orientation(orientation);
            let output = dir.path().join(format!("clean-{}.png", format.ext()));
            remove_metadata(&source, &output).unwrap();
            assert_eq!(image::open(&output).unwrap().to_rgb8(), expected.to_rgb8());
            assert_eq!(std::fs::read(&source).unwrap(), original);
            let mut clean = ImageReader::open(&output)
                .unwrap()
                .with_guessed_format()
                .unwrap()
                .into_decoder()
                .unwrap();
            assert!(clean.exif_metadata().unwrap().is_none());
            assert!(clean.icc_profile().unwrap().is_none());
            // Fresh PNG contains image data only, rather than copied metadata chunks.
            let png = std::fs::read(&output).unwrap();
            let mut at = 8;
            while at < png.len() {
                let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
                assert!([b"IHDR".as_slice(), b"IDAT", b"IEND"].contains(&&png[at + 4..at + 8]));
                at += len + 12;
            }
        }
    }

    #[test]
    fn privacy_rejects_unsupported_or_oversized_images_without_output() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.gif");
        image::RgbImage::new(1, 1).save(&source).unwrap();
        let output = dir.path().join("clean.png");
        assert!(remove_metadata(&source, &output).is_err());
        assert!(!output.exists());
        let source = dir.path().join("wide.png");
        image::RgbImage::new(16_385, 1).save(&source).unwrap();
        assert!(remove_metadata(&source, &output).is_err());
        assert!(!output.exists());
    }

    /// Needs ffmpeg: `FMX_FFMPEG=<path> cargo test webp`.
    #[test]
    fn webp_is_lossy_and_keeps_transparency() {
        let Some(ffmpeg) = std::env::var_os("FMX_FFMPEG").map(std::path::PathBuf::from) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        // Photo-like detail with a transparent corner.
        let photo = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(600, 400, |x, y| {
            let n = (x.wrapping_mul(7919) ^ y.wrapping_mul(104_729)) % 61;
            let alpha = if x < 100 && y < 100 { 0 } else { 255 };
            image::Rgba([(x / 3 + n) as u8, (y / 2 + n) as u8, (n * 4) as u8, alpha])
        }));
        let lossy = dir.path().join("lossy.webp");
        let lossless = dir.path().join("lossless.webp");
        write_image(
            &photo,
            &lossy,
            ImageFormat::Webp,
            &Options::default(),
            Some(&ffmpeg),
        )
        .unwrap();
        write_image(
            &photo,
            &lossless,
            ImageFormat::Webp,
            &Options::default(),
            None,
        )
        .unwrap();
        let size = |path: &Path| std::fs::metadata(path).unwrap().len();
        assert!(
            size(&lossy) * 2 < size(&lossless),
            "{} vs {}",
            size(&lossy),
            size(&lossless)
        );
        let decoded = image::open(&lossy).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (600, 400));
        assert_eq!(decoded.get_pixel(10, 10)[3], 0);
        assert_eq!(decoded.get_pixel(300, 300)[3], 255);
    }
}
