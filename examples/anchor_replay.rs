//! Re-anchors saved engine output without running the engine:
//! `anchor_replay <pdf> <engine.md> <out.md>` finds the mini-pages of every sheet in the PDF,
//! applies the line-number re-anchoring to the matching `<!-- page N -->` section of the
//! Markdown and writes the result (front matter dropped). Used to tune thresholds offline.
use std::path::PathBuf;

use ocr::anchor;
use ocr::vision::{self, Options};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let pdf = PathBuf::from(args.next().expect("pdf"));
    let md = std::fs::read_to_string(args.next().expect("engine md"))?;
    let out = args.next().expect("out md");
    let info = vision::document_info(&pdf)?;
    let body = md.split_once("\n---\n\n").map_or(md.as_str(), |(_, b)| b);
    let mut sections: Vec<(usize, String)> = Vec::new();
    for part in body.split("<!-- page ").skip(1) {
        let (head, text) = part.split_once("-->\n").unwrap_or((part, ""));
        let n: usize = head
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap()
            .parse()?;
        sections.push((n, text.trim_end().to_string()));
    }
    let mut result = String::new();
    for (n, text) in sections {
        let mini = anchor::locate(&pdf, n - 1, &Options::default());
        let applied = anchor::apply(&mini, Some(&text));
        let (text, status) = match applied {
            Some(a) => (a.body, if a.anchored { "anchored" } else { "unverified" }),
            None => (text, "unverified"),
        };
        eprintln!("page {n}/{}: {status}", info.pages);
        result.push_str(&format!("<!-- page {n} -->\n{text}\n\n"));
    }
    std::fs::write(out, result)?;
    Ok(())
}
