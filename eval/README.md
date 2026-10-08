# Evaluation harness (Phase 6)

Python only; uses the spike venv (`spike/.venv/bin/python -I ...`, needs reportlab, pypdfium2,
Pillow, numpy, rapidfuzz). Fixtures are synthetic with exact ground truth.

## Generate
    spike/.venv/bin/python -I eval/generate.py --seed 0 --out eval/out/seed0 [--families pleading,fax]
Families: `pleading`, `contract`, `transcript4up`, `fax`, `table`, `production` (500 pages, about
15 s). Each writes `<family>.pdf` and `<family>.gt.json` (per page: `text`, `tokens`, `bates`,
`kind`, plus `furniture`, the headers/footers/Bates that the scorer ignores). Output is
deterministic per seed. `eval/out/` is gitignored.

## Run and score
    spike/.venv/bin/python -I eval/run.py --engine apple --set eval/out/seed0 --out eval/out/results
    spike/.venv/bin/python -I eval/score.py eval/out/seed0/fax.gt.json eval/out/results/apple/fax.md [--diff-page N]
`run.py` scans each fixture with `target/release/ocr`, scores it, and writes
`<out>/<engine>.json` (merged per family across runs) plus a Markdown table. `apple` uses a private
empty config/state dir. Model engines (`paddleocr-vl`, `olmocr`, `dots-ocr`) need a running server
and use the real config by default; the server's peak footprint is read once from `vmmap` after the
run. Use `--families` to split e.g. production from the rest, `--dpi` and `--extra-args` to tune.
Metrics: CER (whitespace-collapsed, Markdown/HTML table syntax removed, quotes/dashes unified),
token recall, Bates accuracy (page marker), transcript line accuracy (`transcript4up`), pages/s,
peak MB. Pleading margin numbers are stripped from the output before scoring.

## Seed convention
Tune on seeds 0-9 only. Accept on seeds 10-39, which are never looked at while tuning.

## No handwriting family
Trustworthy ground truth for handwriting cannot be generated, so handwritten annotation pages
(PLAN.md section 7) are checked by hand and are not part of this harness.
