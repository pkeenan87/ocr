//! Region detection of one page with debug output: split_page <pdf> <page 1-based>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let path = std::path::Path::new(&a[0]);
    let page: usize = a[1].parse::<usize>().unwrap() - 1;
    let side = ocr::pipeline::side_pass(path, page, 150, &ocr::vision::Options::default()).unwrap();
    for r in &side.regions {
        println!("{:?} {:.3?}", r.kind, r.rect);
    }
}
