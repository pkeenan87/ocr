//! Runs RecognizeDocuments on normalized crops of one page: `crop_probe <pdf> <dpi> x,y,w,h ...`
use std::path::PathBuf;

use ocr::vision::{self, Options};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("pdf"));
    let dpi: u32 = args.next().expect("dpi").parse()?;
    for spec in args {
        let r: Vec<f64> = spec.split(',').map(|v| v.parse().unwrap()).collect();
        let opts = Options {
            crop: Some([r[0], r[1], r[2], r[3]]),
            language_correction: std::env::var("CORR").is_ok(),
            custom_words: std::env::var("WORDS")
                .map(|w| w.split(',').map(String::from).collect())
                .unwrap_or_default(),
            ..Options::default()
        };
        let page = vision::recognize_page(&path, 0, dpi, &opts)?;
        println!("== crop {spec}");
        for b in &page.blocks {
            let text = b.text.clone().unwrap_or_default();
            let items = b
                .items
                .as_ref()
                .map(|i| {
                    i.iter()
                        .map(|x| format!("{} {}", x.marker, x.text))
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .unwrap_or_default();
            let rows = b
                .rows
                .as_ref()
                .map(|r| {
                    r.iter()
                        .map(|c| c.join(" | "))
                        .collect::<Vec<_>>()
                        .join("\n    ")
                })
                .unwrap_or_default();
            println!(
                "[{}] {}{}{}",
                b.kind,
                text.replace('\n', " ⏎ "),
                items,
                rows
            );
        }
    }
    Ok(())
}
