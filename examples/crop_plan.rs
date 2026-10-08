//! Prints the production Phase 2b crop rects of a page (the ones model engines send):
//! `crop_plan <input> <page 1-based> [dpi]`, one `x,y,w,h` per line, in reading order.
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dpi = a.get(3).map_or(200, |d| d.parse().unwrap());
    let side = ocr::pipeline::side_pass(
        Path::new(&a[1]),
        a[2].parse::<usize>().unwrap() - 1,
        dpi,
        &ocr::vision::Options::default(),
    )
    .unwrap();
    eprintln!("columns {} transcript {}", side.columns(), side.transcript);
    for (_, rect) in ocr::model_engine::region_specs(&side).unwrap_or_default() {
        let r = rect.expect("region crops always have a rect");
        println!("{},{},{},{}", r[0], r[1], r[2], r[3]);
    }
}
