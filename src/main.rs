mod vision;

use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    // Phase 0b spike: `ocr <image> [threads]` runs Vision on the image from N Rust threads.
    let mut args = std::env::args().skip(1);
    let Some(image) = args.next().map(PathBuf::from) else {
        println!("ocr — local document OCR (phase 0 spike)\nusage: ocr <image> [threads]");
        return Ok(());
    };
    let threads: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(1);

    let started = Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let image = image.clone();
            std::thread::spawn(move || {
                let t = Instant::now();
                vision::recognize_document(&image).map(|b| (b, t.elapsed()))
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
            items.iter().for_each(|i| println!("    - {i}"));
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
