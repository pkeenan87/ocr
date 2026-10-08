//! Page rendering and document access (`ocr::vision` document functions). On macOS this is the
//! Swift bridge; elsewhere pdfium and the `image` crate (PLAN §9, Phase 7c), which need
//! `OCR_PDFIUM_LIB` (or pdfium in the state dir) and fail loudly without it. The same
//! expectations hold on both, so a Mac run checks the two agree.
//!
//! Fixtures in tests/fixtures: `legal_rot90.pdf` is spike/fixtures/legal.pdf with `/Rotate 90`;
//! `encrypted.pdf` is it with a user password; `fax_g4_two_frames.tif` is a 1-bit CCITT G4 TIFF
//! of legal_200dpi.png at 425x550 whose second frame is the first mirrored.

use std::path::Path;

use ocr::vision::{self, DocKind, GrayImage};

fn gray(path: &str, page: usize, dpi: u32) -> GrayImage {
    vision::render_gray(Path::new(path), page, dpi).unwrap()
}

/// Width and height from a PNG's IHDR.
fn png_size(png: &[u8]) -> (u32, u32) {
    let be = |i: usize| u32::from_be_bytes(png[i..i + 4].try_into().unwrap());
    (be(16), be(20))
}

fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    sum as f64 / a.len() as f64
}

/// `img` turned 90° clockwise.
fn rotate_cw(img: &GrayImage) -> Vec<u8> {
    let (w, h) = (img.width, img.height);
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            // Source (x, y) lands at (h - 1 - y, x) in the h-wide output.
            out[x * h + (h - 1 - y)] = img.pixels[y * w + x];
        }
    }
    out
}

#[test]
fn pdf_pages_render_at_the_requested_dpi() {
    let g = gray("spike/fixtures/legal.pdf", 0, 200);
    assert_eq!((g.width, g.height), (1700, 2200));
    let g = gray("spike/fixtures/legal.pdf", 0, 100);
    assert_eq!((g.width, g.height), (850, 1100));
    // Ink is spread over the page, not squeezed into its top-left corner.
    let dark_rows = (0..g.height)
        .filter(|y| {
            g.pixels[y * g.width..(y + 1) * g.width]
                .iter()
                .any(|p| *p < 128)
        })
        .max()
        .unwrap();
    assert!(dark_rows > g.height * 9 / 10, "last inked row {dark_rows}");
}

#[test]
fn png_sizes_crops_fits_and_rotations() {
    let p = Path::new("spike/fixtures/legal.pdf");
    let size =
        |crop, side, rot| png_size(&vision::render_png_fit(p, 0, 200, crop, side, rot).unwrap());
    assert_eq!(size(None, None, 0), (1700, 2200));
    assert_eq!(size(None, Some(1288), 0), (995, 1288));
    assert_eq!(size(None, Some(1288), 90), (1288, 995));
    assert_eq!(size(Some([0.1, 0.25, 0.5, 0.5]), None, 0), (850, 1100));
    assert_eq!(
        size(Some([0.1, 0.25, 0.5, 0.5]), Some(1000), 90),
        (1000, 773)
    );
}

#[test]
fn rotated_pages_are_turned_clockwise() {
    let upright = gray("spike/fixtures/legal.pdf", 0, 50);
    let rotated = gray("tests/fixtures/legal_rot90.pdf", 0, 50);
    assert_eq!(
        (rotated.width, rotated.height),
        (upright.height, upright.width)
    );
    let diff = mean_abs_diff(&rotated.pixels, &rotate_cw(&upright));
    assert!(diff < 6.0, "mean difference {diff:.2}");
}

#[test]
fn large_images_shrink_to_eleven_inches() {
    // 2550 x 3301 at 200 DPI: longest side 2200.
    let g = gray("spike/fixtures/legal_300dpi.png", 0, 200);
    assert_eq!((g.width, g.height), (1699, 2200));
    let g = gray("spike/fixtures/legal_200dpi.png", 0, 200);
    assert_eq!((g.width, g.height), (1700, 2200));
}

#[test]
fn tiff_frames_are_pages() {
    let info = vision::document_info(Path::new("spike/fixtures/multipage.tif")).unwrap();
    assert_eq!((info.kind, info.pages), (DocKind::Image, 2));
    let fax = "tests/fixtures/fax_g4_two_frames.tif";
    let info = vision::document_info(Path::new(fax)).unwrap();
    assert_eq!((info.kind, info.pages), (DocKind::Image, 2));
    let (a, b) = (gray(fax, 0, 200), gray(fax, 1, 200));
    assert_eq!((a.width, a.height), (425, 550));
    // Mostly white paper, not inverted.
    let mean = a.pixels.iter().map(|p| f64::from(*p)).sum::<f64>() / a.pixels.len() as f64;
    assert!(mean > 200.0, "mean {mean:.1}");
    let mirrored: Vec<u8> = b
        .pixels
        .chunks(b.width)
        .flat_map(|row| row.iter().rev().copied())
        .collect();
    assert_eq!(a.pixels, mirrored);
    assert!(vision::render_gray(Path::new(fax), 2, 200).is_err());
}

#[test]
fn concurrent_renders_agree() {
    let one = gray("spike/fixtures/three_pages.pdf", 1, 100).pixels;
    let handles: Vec<_> = (0..8)
        .map(|_| std::thread::spawn(|| gray("spike/fixtures/three_pages.pdf", 1, 100).pixels))
        .collect();
    for h in handles {
        assert!(h.join().unwrap() == one);
    }
}

#[test]
fn pdf_info_text_and_errors() {
    let info = vision::document_info(Path::new("spike/fixtures/three_pages.pdf")).unwrap();
    assert_eq!((info.kind, info.pages), (DocKind::Pdf, 3));
    let text = vision::text_layer(Path::new("spike/fixtures/legal.pdf"), 0).unwrap();
    assert!(text.contains("Case No."), "{text}");
    assert_eq!(
        vision::text_layer(Path::new("spike/fixtures/legal_200dpi.png"), 0).unwrap(),
        ""
    );
    let err = vision::document_info(Path::new("tests/fixtures/encrypted.pdf")).unwrap_err();
    assert!(err.to_string().contains("PDF is encrypted"), "{err}");
    assert!(vision::render_gray(Path::new("spike/fixtures/legal.pdf"), 1, 100).is_err());
}

/// pdfium against pypdfium2's render of the same page (spike/make_fixture.py): same engine,
/// so the pixels should nearly agree.
#[cfg(not(target_os = "macos"))]
#[test]
fn pdfium_render_matches_the_reference_png() {
    let reference = image::open("spike/fixtures/legal_200dpi.png")
        .unwrap()
        .into_luma8();
    let g = gray("spike/fixtures/legal.pdf", 0, 200);
    assert_eq!((g.width as u32, g.height as u32), reference.dimensions());
    let diff = mean_abs_diff(&g.pixels, reference.as_raw());
    assert!(diff < 2.0, "mean difference {diff:.2}");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn heic_is_refused_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("photo.heic");
    std::fs::write(&path, b"\0\0\0\x18ftypheic\0\0\0\0mif1heic").unwrap();
    let err = vision::document_info(&path).unwrap_err();
    assert!(err.to_string().contains("HEIC"), "{err}");
}
