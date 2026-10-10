//! Documents off macOS (PLAN §9, Phase 7c): PDFs through pdfium, images through the `image` and
//! `tiff` crates. Mirrors the Swift bridge's rendering (`bridge/`, `renderPDFPage`, `loadImage`,
//! `ocr_bridge_render_png`) so page sizes, crops and rotations come out the same.

use std::fs::File;
use std::io::{BufReader, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use anyhow::{Context, Result, anyhow, bail};
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage as Luma8, ImageFormat, RgbImage, RgbaImage};
use pdfium_render::prelude::*;

use super::{CharCounts, DocInfo, DocKind, GrayImage};

/// Largest bitmap side we will render, to bound memory on oversized page boxes (as the bridge).
const MAX_SIDE: f64 = 16384.0;

/// The pdfium release `pdfium-render`'s `pdfium_7881` API feature matches
/// (github.com/bblanchon/pdfium-binaries, tag `chromium/7881`).
pub const PDFIUM_RELEASE: &str = "chromium/7881";

// ---------------------------------------------------------------- pdfium

/// pdfium is single-threaded. `pdfium-render`'s `thread_safe` feature locks each call, but
/// documents opened and rendered from several threads at once still crash it, so every
/// operation holds this lock from open to close: PDF pages render one at a time.
static PDFIUM: OnceLock<Result<Mutex<Pdfium>, String>> = OnceLock::new();

/// Where the pdfium library is looked for, in order: `$OCR_PDFIUM_LIB`, the state dir's
/// `pdfium/` (where setup installs it), next to the executable, then the system library path.
fn pdfium_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(p) = std::env::var_os("OCR_PDFIUM_LIB") {
        paths.push(PathBuf::from(p));
    }
    if let Ok(state) = crate::config::state_dir() {
        paths.push(Pdfium::pdfium_platform_library_name_at_path(
            &state.join("pdfium"),
        ));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        paths.push(Pdfium::pdfium_platform_library_name_at_path(&dir));
    }
    paths
}

fn bind_pdfium() -> Result<Pdfium, String> {
    let candidates = pdfium_candidates();
    if let Some(path) = candidates.iter().find(|p| p.is_file()) {
        return Pdfium::bind_to_library(path)
            .map(Pdfium::new)
            .map_err(|e| format!("cannot load pdfium from {}: {e}", path.display()));
    }
    if let Ok(bindings) = Pdfium::bind_to_system_library() {
        return Ok(Pdfium::new(bindings));
    }
    let tried: Vec<String> = candidates
        .iter()
        .map(|p| format!("  {}", p.display()))
        .chain([format!(
            "  {} on the system library path",
            Pdfium::pdfium_platform_library_name().to_string_lossy()
        )])
        .collect();
    Err(format!(
        "pdfium library not found ({PDFIUM_RELEASE} from github.com/bblanchon/pdfium-binaries); tried:\n{}\n\
         Set OCR_PDFIUM_LIB to its path.",
        tried.join("\n")
    ))
}

/// Exclusive use of pdfium; keep the guard alive until every document from it is dropped.
fn pdfium() -> Result<MutexGuard<'static, Pdfium>> {
    let lock = PDFIUM
        .get_or_init(|| bind_pdfium().map(Mutex::new))
        .as_ref()
        .map_err(|e| anyhow!("{e}"))?;
    Ok(lock.lock().unwrap_or_else(PoisonError::into_inner))
}

fn open_pdf<'a>(pdfium: &'a Pdfium, path: &Path) -> Result<PdfDocument<'a>> {
    pdfium.load_pdf_from_file(path, None).map_err(|e| match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            anyhow!("PDF is encrypted")
        }
        e => anyhow!("cannot open PDF: {e}"),
    })
}

fn pdf_page<'a>(doc: &'a PdfDocument, index: usize) -> Result<PdfPage<'a>> {
    let pages = doc.pages();
    let n = pages.len().max(0) as usize;
    if index >= n {
        bail!("page {} out of range", index + 1);
    }
    pages
        .get(index as PdfPageIndex)
        .map_err(|e| anyhow!("page {}: {e}", index + 1))
}

/// Renders a PDF page over white at `dpi`, or, with `target_side`, scaled so the longest side
/// of `crop` (normalized top-left [x, y, w, h], or the whole page) is that many pixels. The
/// size is computed in points and rounded per axis, as the bridge's `renderPDFPage`.
fn render_pdf(
    path: &Path,
    index: usize,
    dpi: u32,
    target_side: Option<u32>,
    crop: Option<[f64; 4]>,
) -> Result<RgbImage> {
    let pdfium = pdfium()?;
    let doc = open_pdf(&pdfium, path)?;
    let page = pdf_page(&doc, index)?;
    // pdfium reports the displayed size: /Rotate is already applied to width and height.
    let (pw, ph) = (page.width().value as f64, page.height().value as f64);
    if pw <= 0.0 || ph <= 0.0 {
        bail!("empty page box");
    }
    let (w, h) = pdf_pixel_size(pw, ph, dpi, target_side, crop);
    let config = PdfRenderConfig::new()
        // Target (not fixed) sizes: pdfium-render scales the page to them, and the per-axis
        // scales come out as the bridge's (w / pw, h / ph). A fixed size renders at 72 DPI.
        .set_target_size(w as Pixels, h as Pixels)
        .set_clear_color(PdfColor::WHITE)
        // The bridge draws page content only (`CGContext.drawPDFPage`), not annotations.
        .render_annotations(false)
        .render_form_data(false);
    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| anyhow!("render failed: {e}"))?;
    let image = bitmap
        .as_image()
        .map_err(|e| anyhow!("render failed: {e}"))?;
    Ok(flatten(image))
}

/// Pixel size of a rendered PDF page of `pw` x `ph` points (see `render_pdf`).
fn pdf_pixel_size(
    pw: f64,
    ph: f64,
    dpi: u32,
    target_side: Option<u32>,
    crop: Option<[f64; 4]>,
) -> (u32, u32) {
    let mut scale = f64::from(dpi) / 72.0;
    if let Some(side) = target_side {
        let (cw, ch) = crop.map_or((pw, ph), |c| (c[2] * pw, c[3] * ph));
        scale = f64::from(side) / cw.max(ch).max(1e-6);
    }
    scale = scale.min(MAX_SIDE / pw.max(ph));
    let w = ((pw * scale).round() as u32).max(1);
    let h = ((ph * scale).round() as u32).max(1);
    (w, h)
}

// ---------------------------------------------------------------- files

fn is_pdf(path: &Path) -> bool {
    let mut head = [0u8; 5];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| &head == b"%PDF-")
}

/// HEIF/HEIC (`ftyp` brand heic/heix/mif1/...): admitted by `input.rs`, decodable only on macOS.
fn is_heif(path: &Path) -> bool {
    let mut head = [0u8; 12];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| &head[4..8] == b"ftyp")
}

fn is_tiff(path: &Path) -> bool {
    let mut head = [0u8; 4];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| &head == b"II*\0" || &head == b"MM\0*")
}

fn tiff_decoder(path: &Path) -> Result<tiff::decoder::Decoder<BufReader<File>>> {
    let file = File::open(path).context("cannot open image")?;
    tiff::decoder::Decoder::new(BufReader::new(file)).context("cannot open image")
}

fn tiff_frames(path: &Path) -> Result<usize> {
    let mut dec = tiff_decoder(path)?;
    let mut n = 1;
    while dec.more_images() {
        dec.next_image().context("cannot read TIFF frame")?;
        n += 1;
    }
    Ok(n)
}

/// Decodes TIFF frame `index` (multi-page scans), WhiteIsZero already inverted by the crate.
fn tiff_frame(path: &Path, index: usize) -> Result<DynamicImage> {
    use tiff::ColorType as C;
    use tiff::decoder::DecodingResult as D;
    let mut dec = tiff_decoder(path)?;
    dec.seek_to_image(index)
        .map_err(|_| anyhow!("image frame {} out of range", index + 1))?;
    let (w, h) = dec.dimensions().context("cannot read TIFF frame")?;
    let color = dec.colortype().context("cannot read TIFF frame")?;
    let data = dec.read_image().context("cannot decode TIFF frame")?;
    let bad = || anyhow!("unsupported TIFF pixel format {color:?}");
    let image = match (color, data) {
        (C::Gray(1), D::U8(bits)) => {
            let stride = (w as usize).div_ceil(8);
            let pixels = (0..h as usize)
                .flat_map(|y| (0..w as usize).map(move |x| (y, x)))
                .map(|(y, x)| {
                    if bits[y * stride + x / 8] & (0x80 >> (x % 8)) != 0 {
                        255
                    } else {
                        0
                    }
                })
                .collect();
            DynamicImage::ImageLuma8(Luma8::from_raw(w, h, pixels).ok_or_else(bad)?)
        }
        (C::Gray(8), D::U8(v)) => {
            DynamicImage::ImageLuma8(Luma8::from_raw(w, h, v).ok_or_else(bad)?)
        }
        (C::Gray(16), D::U16(v)) => {
            DynamicImage::ImageLuma16(image::ImageBuffer::from_raw(w, h, v).ok_or_else(bad)?)
        }
        (C::GrayA(8), D::U8(v)) => {
            DynamicImage::ImageLumaA8(image::ImageBuffer::from_raw(w, h, v).ok_or_else(bad)?)
        }
        (C::RGB(8), D::U8(v)) => {
            DynamicImage::ImageRgb8(RgbImage::from_raw(w, h, v).ok_or_else(bad)?)
        }
        (C::RGB(16), D::U16(v)) => {
            DynamicImage::ImageRgb16(image::ImageBuffer::from_raw(w, h, v).ok_or_else(bad)?)
        }
        (C::RGBA(8), D::U8(v)) => {
            DynamicImage::ImageRgba8(RgbaImage::from_raw(w, h, v).ok_or_else(bad)?)
        }
        (C::RGBA(16), D::U16(v)) => {
            DynamicImage::ImageRgba16(image::ImageBuffer::from_raw(w, h, v).ok_or_else(bad)?)
        }
        (C::CMYK(8), D::U8(v)) => {
            let rgb = v
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    let k = 255 - u32::from(p[3]);
                    [0, 1, 2].map(|i| ((255 - u32::from(p[i])) * k / 255) as u8)
                })
                .collect();
            DynamicImage::ImageRgb8(RgbImage::from_raw(w, h, rgb).ok_or_else(bad)?)
        }
        _ => return Err(bad()),
    };
    Ok(image)
}

/// Decodes image `path`, frame `index` (only TIFF has more than one).
fn decode_image(path: &Path, index: usize) -> Result<DynamicImage> {
    if is_heif(path) {
        bail!("HEIC images are not supported on this platform; convert to PNG or JPEG");
    }
    if is_tiff(path) {
        return tiff_frame(path, index);
    }
    if index != 0 {
        bail!("image frame {} out of range", index + 1);
    }
    let bytes = std::fs::read(path).context("cannot open image")?;
    let format = image::guess_format(&bytes).context("cannot open image")?;
    image::load(Cursor::new(bytes), format).context("cannot decode image")
}

/// Image frame `index` over white. Unless `fit_to_side`, an image longer than 11 inches at `dpi`
/// is downscaled to that, as the bridge's `loadImage`.
fn load_image(path: &Path, index: usize, dpi: u32, fit_to_side: bool) -> Result<RgbImage> {
    let image = flatten(decode_image(path, index)?);
    match downscaled_size(image.width(), image.height(), dpi, fit_to_side) {
        Some((w, h)) => Ok(image::imageops::resize(&image, w, h, FilterType::Lanczos3)),
        None => Ok(image),
    }
}

/// The size `load_image` scales a `w` x `h` image to, if it scales it at all.
fn downscaled_size(w: u32, h: u32, dpi: u32, fit_to_side: bool) -> Option<(u32, u32)> {
    let longest = f64::from(w.max(h));
    let target = 11.0 * f64::from(dpi);
    if fit_to_side || longest <= target {
        return None;
    }
    let scale = target / longest;
    Some((
        ((f64::from(w) * scale).round() as u32).max(1),
        ((f64::from(h) * scale).round() as u32).max(1),
    ))
}

/// Composites over white and drops alpha (the bridge draws into a white RGB context).
fn flatten(image: DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.into_rgb8();
    }
    let rgba = image.into_rgba8();
    let (w, h) = rgba.dimensions();
    let rgb = rgba
        .pixels()
        .flat_map(|p| {
            let a = u32::from(p[3]);
            [0, 1, 2].map(|i| ((u32::from(p[i]) * a + 255 * (255 - a) + 127) / 255) as u8)
        })
        .collect();
    RgbImage::from_raw(w, h, rgb).expect("size matches")
}

/// `CGRect.integral` of a normalized top-left crop on a `w` x `h` image: origin floored, far
/// edge ceiled, clamped to the image. `None` if nothing is left.
fn crop_pixels(crop: [f64; 4], w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
    let (fw, fh) = (f64::from(w), f64::from(h));
    let x0 = (crop[0] * fw).floor().clamp(0.0, fw);
    let y0 = (crop[1] * fh).floor().clamp(0.0, fh);
    let x1 = ((crop[0] + crop[2]) * fw).ceil().clamp(0.0, fw);
    let y1 = ((crop[1] + crop[3]) * fh).ceil().clamp(0.0, fh);
    (x1 > x0 && y1 > y0).then_some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Size that puts exactly `side` pixels on the longest side (the bridge's `fitLongestSide`).
fn fit_size(w: u32, h: u32, side: u32) -> (u32, u32) {
    let longest = w.max(h);
    if longest == side {
        return (w, h);
    }
    let scale = f64::from(side) / f64::from(longest);
    let fw = if w >= h {
        side
    } else {
        ((f64::from(w) * scale).round() as u32).max(1)
    };
    let fh = if h > w {
        side
    } else {
        ((f64::from(h) * scale).round() as u32).max(1)
    };
    (fw, fh)
}

/// Turns counter-clockwise by `degrees` (0, 90, 180 or 270), as PIL's `transpose(ROTATE_*)`.
fn rotate_ccw(image: RgbImage, degrees: u32) -> RgbImage {
    match degrees % 360 / 90 {
        1 => image::imageops::rotate270(&image),
        2 => image::imageops::rotate180(&image),
        3 => image::imageops::rotate90(&image),
        _ => image,
    }
}

fn render(path: &Path, index: usize, dpi: u32) -> Result<RgbImage> {
    if is_pdf(path) {
        render_pdf(path, index, dpi, None, None)
    } else {
        load_image(path, index, dpi, false)
    }
}

// ---------------------------------------------------------------- page functions

/// Page count and kind (PDF or image). Multi-page TIFF reports its frame count.
pub fn document_info(path: &Path) -> Result<DocInfo> {
    if is_pdf(path) {
        let pdfium = pdfium()?;
        let doc = open_pdf(&pdfium, path)?;
        return Ok(DocInfo {
            kind: DocKind::Pdf,
            pages: doc.pages().len().max(0) as usize,
        });
    }
    let pages = if is_tiff(path) {
        tiff_frames(path)?
    } else {
        if is_heif(path) {
            bail!("HEIC images are not supported on this platform; convert to PNG or JPEG");
        }
        let bytes = std::fs::read(path).context("cannot open image")?;
        image::guess_format(&bytes).map_err(|_| anyhow!("cannot open image"))?;
        1
    };
    Ok(DocInfo {
        kind: DocKind::Image,
        pages,
    })
}

/// Embedded text of a PDF page (0-based index); empty for images. pdfium's spacing and order
/// can differ from PDFKit's `page.string`.
/// Embedded text by region is only used on the Mac (with Apple Vision's layout); elsewhere the
/// layout-preserving text-layer path is not available.
pub fn text_in_regions(
    _path: &Path,
    _page_index: usize,
    _regions: &[[f64; 4]],
    _geometric: bool,
) -> Result<Vec<String>> {
    anyhow::bail!("text by region needs Apple Vision layout (macOS)")
}

/// Characters of a PDF page's text drawn visibly and invisibly, form XObjects included. A text
/// object is invisible in a mode that neither fills nor strokes (invisible, or clip only) or with
/// a fill alpha of 0. Anything pdfium cannot tell is an error, which callers treat as unknown.
pub(super) fn char_counts(path: &Path, page_index: usize) -> Result<CharCounts> {
    let pdfium = pdfium()?;
    let doc = open_pdf(&pdfium, path)?;
    let page = pdf_page(&doc, page_index)?;
    let mut counts = CharCounts::default();
    count_chars(page.objects().iter(), 0, &mut counts)?;
    Ok(counts)
}

/// Deepest nesting of form XObjects entered.
const MAX_FORM_DEPTH: usize = 8;

fn count_chars<'a>(
    objects: impl Iterator<Item = PdfPageObject<'a>>,
    depth: usize,
    counts: &mut CharCounts,
) -> Result<()> {
    for object in objects {
        if let Some(text) = object.as_text_object() {
            let n = text.text().chars().count();
            let hidden_mode = !matches!(
                text.render_mode(),
                PdfPageTextRenderMode::FilledUnstroked
                    | PdfPageTextRenderMode::StrokedUnfilled
                    | PdfPageTextRenderMode::FilledThenStroked
                    | PdfPageTextRenderMode::FilledUnstrokedClipping
                    | PdfPageTextRenderMode::StrokedUnfilledClipping
                    | PdfPageTextRenderMode::FilledThenStrokedClipping
            );
            let fill = text
                .fill_color()
                .map_err(|e| anyhow!("text fill colour: {e}"))?;
            // An unrecognised mode (`Unknown`) counts as hidden: not a guess that it is visible.
            if hidden_mode || fill.alpha() == 0 {
                counts.invisible += n;
            } else {
                counts.shown += n;
            }
        } else if let Some(form) = object.as_x_object_form_object() {
            if depth >= MAX_FORM_DEPTH {
                bail!("form XObjects nested too deeply");
            }
            count_chars(form.iter(), depth + 1, counts)?;
        }
    }
    Ok(())
}

pub fn text_layer(path: &Path, page_index: usize) -> Result<String> {
    if !is_pdf(path) {
        return Ok(String::new());
    }
    let pdfium = pdfium()?;
    let doc = open_pdf(&pdfium, path)?;
    let page = pdf_page(&doc, page_index)?;
    let text = page.text().map_err(|e| anyhow!("text layer: {e}"))?.all();
    Ok(text)
}

/// Render page `page_index` (0-based) at `dpi` as grayscale, row 0 at the top.
pub fn render_gray(path: &Path, page_index: usize, dpi: u32) -> Result<GrayImage> {
    let rgb = render(path, page_index, dpi)
        .with_context(|| format!("cannot render page {} as grayscale", page_index + 1))?;
    let gray = DynamicImage::ImageRgb8(rgb).into_luma8();
    Ok(GrayImage {
        width: gray.width() as usize,
        height: gray.height() as usize,
        pixels: gray.into_raw(),
    })
}

/// Renders a page as PNG bytes, as the bridge's `ocr_bridge_render_png`: optionally cropped to a
/// normalized top-left [x, y, w, h]; `max_side` ignores `dpi` and makes the image (the crop, if
/// any) exactly that many pixels on its longest side; `rotation` then turns it counter-clockwise.
pub fn render_png_fit(
    path: &Path,
    page_index: usize,
    dpi: u32,
    crop: Option<[f64; 4]>,
    max_side: Option<u32>,
    rotation: u32,
) -> Result<Vec<u8>> {
    let fail = || format!("cannot render page {} as PNG", page_index + 1);
    let mut image = if is_pdf(path) {
        render_pdf(path, page_index, dpi, max_side, crop)
    } else {
        load_image(path, page_index, dpi, max_side.is_some())
    }
    .with_context(fail)?;
    if let Some(c) = crop {
        let (x, y, w, h) =
            crop_pixels(c, image.width(), image.height()).ok_or_else(|| anyhow!(fail()))?;
        image = image::imageops::crop_imm(&image, x, y, w, h).to_image();
    }
    if let Some(side) = max_side {
        let (w, h) = fit_size(image.width(), image.height(), side);
        if (w, h) != image.dimensions() {
            image = image::imageops::resize(&image, w, h, FilterType::Lanczos3);
        }
    }
    let image = rotate_ccw(image, rotation);
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .with_context(fail)?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_size_matches_the_bridge() {
        // US Letter at 200 DPI; at 1288 px on the longest side; the top half (612 x 396 pt) at
        // 1000 px on its longest side; capped at 16384.
        assert_eq!(pdf_pixel_size(612.0, 792.0, 200, None, None), (1700, 2200));
        assert_eq!(
            pdf_pixel_size(612.0, 792.0, 200, Some(1288), None),
            (995, 1288)
        );
        assert_eq!(
            pdf_pixel_size(612.0, 792.0, 200, Some(1000), Some([0.0, 0.0, 1.0, 0.5])),
            (1000, 1294)
        );
        assert_eq!(pdf_pixel_size(612.0, 7920.0, 400, None, None).1, 16384);
    }

    #[test]
    fn images_shrink_only_past_eleven_inches() {
        assert_eq!(downscaled_size(1700, 2200, 200, false), None);
        assert_eq!(downscaled_size(2550, 3300, 200, false), Some((1700, 2200)));
        assert_eq!(downscaled_size(2550, 3300, 200, true), None);
    }

    #[test]
    fn crop_is_integral() {
        assert_eq!(
            crop_pixels([0.1, 0.25, 0.5, 0.5], 1001, 2000),
            Some((100, 500, 501, 1000))
        );
        assert_eq!(
            crop_pixels([0.0, 0.0, 1.0, 1.0], 10, 20),
            Some((0, 0, 10, 20))
        );
        assert_eq!(crop_pixels([1.0, 0.0, 0.5, 1.0], 10, 20), None);
    }

    #[test]
    fn fit_puts_side_on_the_longest_edge() {
        assert_eq!(fit_size(1700, 2200, 1288), (995, 1288));
        assert_eq!(fit_size(2200, 1700, 1288), (1288, 995));
        assert_eq!(fit_size(500, 500, 1000), (1000, 1000));
        assert_eq!(fit_size(1288, 10, 1288), (1288, 10));
    }

    #[test]
    fn rotation_is_counter_clockwise() {
        // A 2x1 image, red on the left: turned 90° CCW, red ends up at the bottom.
        let mut img = RgbImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        let r = rotate_ccw(img.clone(), 90);
        assert_eq!(r.dimensions(), (1, 2));
        assert_eq!(r.get_pixel(0, 1)[0], 255);
        let r = rotate_ccw(img, 270);
        assert_eq!(r.get_pixel(0, 0)[0], 255);
    }

    #[test]
    fn alpha_flattens_onto_white() {
        let img = RgbaImage::from_raw(2, 1, vec![0, 0, 0, 0, 0, 0, 0, 255]).unwrap();
        let rgb = flatten(DynamicImage::ImageRgba8(img));
        assert_eq!(rgb.get_pixel(0, 0).0, [255, 255, 255]);
        assert_eq!(rgb.get_pixel(1, 0).0, [0, 0, 0]);
    }
}
