//! Writes a page through the production render path: `render_png <file> <page> <dpi> <out.png> [x,y,w,h]`
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let crop = a.get(4).map(|s| {
        let v: Vec<f64> = s.split(',').map(|x| x.parse().unwrap()).collect();
        [v[0], v[1], v[2], v[3]]
    });
    let png = ocr::vision::render_png(
        &PathBuf::from(&a[0]),
        a[1].parse::<usize>()? - 1,
        a[2].parse()?,
        crop,
    )?;
    std::fs::write(&a[3], png)?;
    Ok(())
}
