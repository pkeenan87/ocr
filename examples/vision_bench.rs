use ocr::vision::{self, Options};

use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    // Reproduces docs/spike.md:
    //   cargo run --release --example vision_bench -- <image> [threads]   thread scaling
    //   cargo run --release --example vision_bench -- <image> matrix      recognition-option matrix
    //   cargo run --release --example vision_bench -- <file> dpi-matrix   DPI x correction matrix
    //                                                  through the production render path
    let mut args = std::env::args().skip(1);
    let Some(image) = args.next().map(PathBuf::from) else {
        println!("usage: vision_bench <image> [threads|matrix]");
        return Ok(());
    };
    match args.next().as_deref() {
        Some("matrix") => return matrix(&image),
        Some("dpi-matrix") => return dpi_matrix(&image),
        _ => {}
    }
    let mut args = std::env::args().skip(2);
    let threads: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(1);

    let started = Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let image = image.clone();
            std::thread::spawn(move || {
                let t = Instant::now();
                vision::recognize_document(&image, &Options::default()).map(|b| (b, t.elapsed()))
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Result<_, _>>()?;

    let (blocks, _) = &results[0];
    for b in blocks {
        println!(
            "[{}] {:?}",
            b.kind,
            b.bbox.map(|v| (v * 1000.0).round() / 1000.0)
        );
        if let Some(t) = &b.text {
            println!("    {}", t.replace('\n', "\n    "));
        }
        if let Some(items) = &b.items {
            items
                .iter()
                .for_each(|i| println!("    {} {}", i.marker, i.text));
        }
        if let Some(rows) = &b.rows {
            rows.iter()
                .for_each(|r| println!("    | {} |", r.join(" | ")));
        }
    }
    let first = serde_json::to_string(
        &results[0]
            .0
            .iter()
            .map(|b| (&b.kind, &b.text, &b.items, &b.rows))
            .collect::<Vec<_>>(),
    )?;
    let identical = results.iter().all(|(b, _)| {
        serde_json::to_string(
            &b.iter()
                .map(|b| (&b.kind, &b.text, &b.items, &b.rows))
                .collect::<Vec<_>>(),
        )
        .unwrap()
            == first
    });
    let per_call: Vec<_> = results
        .iter()
        .map(|(_, d)| format!("{:.2}s", d.as_secs_f64()))
        .collect();
    eprintln!(
        "threads={threads} wall={:.2}s per_call={per_call:?} identical={identical}",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn matrix(image: &std::path::Path) -> anyhow::Result<()> {
    let checks = [
        "3:26-cv-04417",
        "$22,375.50",
        "$48,250.00",
        "$51,900.00",
        "$122,525.50",
        "SMITH000123",
    ];
    for (langs, auto) in [(vec!["en-US"], false), (vec![], true)] {
        for correction in [true, false] {
            let opts = Options {
                languages: langs.iter().map(|s| s.to_string()).collect(),
                auto_detect_language: auto,
                language_correction: correction,
                custom_words: Vec::new(),
                crop: None,
                lines: false,
                reader: None,
            };
            let blocks = vision::recognize_document(image, &opts)?;
            let all: String = blocks
                .iter()
                .flat_map(|b| {
                    let mut v = b.text.clone().into_iter().collect::<Vec<_>>();
                    v.extend(
                        b.items
                            .iter()
                            .flatten()
                            .map(|i| format!("{} {}", i.marker, i.text)),
                    );
                    v.extend(b.rows.clone().unwrap_or_default().into_iter().flatten());
                    v
                })
                .collect::<Vec<_>>()
                .join("\n");
            let missing: Vec<_> = checks.iter().filter(|c| !all.contains(**c)).collect();
            println!("langs={langs:?} auto={auto} correction={correction} missing={missing:?}");
        }
    }
    Ok(())
}

const CHECKS: [&str; 6] = [
    "3:26-cv-04417",
    "$22,375.50",
    "$48,250.00",
    "$51,900.00",
    "$122,525.50",
    "SMITH000123",
];

/// Exact-token check at every DPI, language correction on and off, via `recognize_page`.
fn dpi_matrix(file: &std::path::Path) -> anyhow::Result<()> {
    println!("{}", file.display());
    for dpi in [150, 200, 250, 300] {
        for correction in [true, false] {
            let opts = Options {
                language_correction: correction,
                ..Options::default()
            };
            let page = vision::recognize_page(file, 0, dpi, &opts)?;
            let md = ocr::markdown::render_blocks(&page.blocks).0;
            let all = format!(
                "{md}\n{}",
                page.blocks
                    .iter()
                    .flat_map(|b| b.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            let missing: Vec<_> = CHECKS.iter().filter(|c| !all.contains(**c)).collect();
            let verdict = if missing.is_empty() {
                "all ok".to_string()
            } else {
                format!("misses {missing:?}")
            };
            println!("dpi={dpi} correction={correction}: {verdict}");
        }
    }
    Ok(())
}
