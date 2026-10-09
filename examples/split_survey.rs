//! Lists pages whose region detection splits into columns: split_survey <pdf>... [--dpi N]
//! Output: <file> <page> <columns> <column rects>

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = ocr::vision::Options::default();
    for a in &args {
        let path = Path::new(a);
        let n = ocr::vision::document_info(path)
            .map_or(0, |i| i.pages)
            .min(60);
        let mut split = 0;
        for page in 0..n {
            let side = match ocr::pipeline::side_pass(path, page, 150, &opts) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if side.columns() >= 2 {
                split += 1;
                let cols: Vec<String> = side
                    .regions
                    .iter()
                    .map(|r| format!("{:?}{:.3?}", r.kind, r.rect))
                    .collect();
                println!(
                    "{} p{} {}",
                    path.file_stem().unwrap().to_string_lossy(),
                    page + 1,
                    cols.join(" ")
                );
            }
        }
        eprintln!("{}: {split}/{n} split", path.display());
    }
}
