//! Prints the column regions of a page: dump_regions <input> <page> [dpi].

use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = Path::new(&a[1]);
    let page: usize = a[2].parse().unwrap();
    let dpi = a.get(3).map_or(100, |d| d.parse().unwrap());
    let opts = ocr::vision::Options::default();
    let first = ocr::vision::recognize_page(path, page, 200, &opts).unwrap();
    let tables: Vec<[f64; 4]> = first
        .blocks
        .iter()
        .filter(|b| b.kind == "table")
        .map(|b| b.bbox)
        .collect();
    let img = ocr::vision::render_gray(path, page, dpi).unwrap();
    for r in ocr::regions::detect(&img, &tables) {
        println!("{:?} {:.3?}", r.kind, r.rect);
    }
}
