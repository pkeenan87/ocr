//! Vision blocks next to the embedded text in the same boxes: region_text <pdf> <page 1-based>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let path = std::path::Path::new(&a[0]);
    let index = a[1].parse::<usize>().unwrap() - 1;
    let page =
        ocr::vision::recognize_page(path, index, 150, &ocr::vision::Options::default()).unwrap();
    let boxes: Vec<[f64; 4]> = page.blocks.iter().map(|b| b.bbox).collect();
    let texts =
        ocr::vision::text_in_regions(path, index, &boxes, std::env::var_os("GEOMETRIC").is_some())
            .unwrap();
    for (b, t) in page.blocks.iter().zip(texts) {
        let v = b.text.clone().unwrap_or_else(|| format!("<{}>", b.kind));
        println!(
            "[{}] {:.3?}\n  vision:   {}\n  embedded: {}",
            b.kind,
            b.bbox,
            v.replace('\n', " "),
            t.replace('\n', " ")
        );
    }
}
