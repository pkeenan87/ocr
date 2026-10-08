//! Prints, per page, the render sizes and the column regions found without table masks, for
//! comparing the macOS bridge with pdfium (PLAN §9, Phase 7c):
//! `region_report <file>...` (defaults to the layout fixtures). Run it on both platforms and
//! diff the output.

use std::path::Path;

const FIXTURES: &[&str] = &[
    "spike/fixtures/legal.pdf",
    "spike/fixtures/legal_scan.pdf",
    "spike/fixtures/two_column.pdf",
    "spike/fixtures/three_pages.pdf",
    "spike/fixtures/four_up.pdf",
    "spike/fixtures/transcript_full.pdf",
    "spike/fixtures/narrow_unequal_seed2.pdf",
    "spike/fixtures/multipage.tif",
    "spike/fixtures/legal_scan_rot90.png",
    "spike/heldout/block_quote.pdf",
    "spike/heldout/four_up.pdf",
    "spike/heldout/lower_half_columns.pdf",
    "spike/heldout/narrow_unequal.pdf",
    "spike/heldout/signature_block.pdf",
];

fn png_size(png: &[u8]) -> (u32, u32) {
    let be = |i: usize| u32::from_be_bytes(png[i..i + 4].try_into().unwrap());
    (be(16), be(20))
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let files: Vec<&str> = if args.is_empty() {
        FIXTURES.to_vec()
    } else {
        args.iter().map(String::as_str).collect()
    };
    for file in files {
        let path = Path::new(file);
        let info = ocr::vision::document_info(path)?;
        println!("{file}: {:?}, {} page(s)", info.kind, info.pages);
        for page in 0..info.pages.min(3) {
            let gray = ocr::vision::render_gray(path, page, 100)?;
            let png200 = ocr::vision::render_png(path, page, 200, None)?;
            let fit = ocr::vision::render_png_fit(path, page, 200, None, Some(1288), 0)?;
            let crop = ocr::vision::render_png_fit(
                path,
                page,
                200,
                Some([0.1, 0.25, 0.5, 0.5]),
                Some(1000),
                90,
            )?;
            println!(
                "  page {}: gray@100 {}x{}, png@200 {:?}, fit1288 {:?}, crop+fit1000+rot90 {:?}",
                page + 1,
                gray.width,
                gray.height,
                png_size(&png200),
                png_size(&fit),
                png_size(&crop)
            );
            for r in ocr::regions::detect(&gray, &[]) {
                println!(
                    "    {:?} [{:.3}, {:.3}, {:.3}, {:.3}]",
                    r.kind, r.rect[0], r.rect[1], r.rect[2], r.rect[3]
                );
            }
        }
    }
    Ok(())
}
