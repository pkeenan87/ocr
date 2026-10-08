//! Writes a page through the production render path:
//! `render_png <file> <page> <dpi> <out.png> [x,y,w,h|-] [max_side] [rotation]`
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let crop = a.get(4).filter(|s| *s != "-").map(|s| {
        let v: Vec<f64> = s.split(',').map(|x| x.parse().unwrap()).collect();
        [v[0], v[1], v[2], v[3]]
    });
    let max_side = a.get(5).map(|s| s.parse()).transpose()?;
    let rotation = a.get(6).map_or(Ok(0), |s| s.parse())?;
    let png = ocr::vision::render_png_fit(
        &PathBuf::from(&a[0]),
        a[1].parse::<usize>()? - 1,
        a[2].parse()?,
        crop,
        max_side,
        rotation,
    )?;
    std::fs::write(&a[3], png)?;
    Ok(())
}
