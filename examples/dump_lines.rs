//! Prints Vision's line-level observations of one page: `dump_lines <pdf> <page> <dpi> [x,y,w,h]`.
//! `READER=text` uses the second reader (`RecognizeTextRequest`) and prints its confidence.
use std::path::PathBuf;

use ocr::vision::{self, Options};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("pdf"));
    let page: usize = args.next().expect("page (1-based)").parse()?;
    let dpi: u32 = args.next().expect("dpi").parse()?;
    let crop = args.next().map(|c| {
        let v: Vec<f64> = c.split(',').map(|x| x.parse().unwrap()).collect();
        [v[0], v[1], v[2], v[3]]
    });
    let opts = Options {
        crop,
        lines: true,
        reader: std::env::var_os("READER").map(|_| "text"),
        ..Options::default()
    };
    let page = vision::recognize_page(&path, page - 1, dpi, &opts)?;
    for l in &page.lines {
        let b = l.bbox;
        println!(
            "x={:.4} y={:.4} w={:.4} h={:.4} c={:?}  {} {:?}",
            b[0], b[1], b[2], b[3], l.confidence, l.text, l.alternatives
        );
    }
    Ok(())
}
