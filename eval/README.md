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
    spike/.venv/bin/python -I eval/run.py --engine apple --set eval/out/seed10 eval/out/seed11 --out eval/out/results [--tag NAME]
    spike/.venv/bin/python -I eval/score.py eval/out/seed0/fax.gt.json eval/out/results/apple/seed0/fax.md [--diff-page N]
`run.py` scans each fixture of each `--set` with `target/release/ocr`, scores it, and writes
`<out>/<tag>.json` (per family and seed, merged with earlier runs of the same tag; `--tag` defaults to
the engine, use one tag per DPI when tuning) plus an aggregated Markdown table (micro-averaged over
the sets of that invocation). Every engine runs in the ISOLATED state `eval/out/state/{config,state}`
(`OCR_CONFIG_DIR`/`OCR_STATE_DIR`), never the real `~/.config/ocr`; `--use-real-config` is opt-in.
Model engines need a server in that state:

    export OCR_CONFIG_DIR=$PWD/eval/out/state/config OCR_STATE_DIR=$PWD/eval/out/state/state
    target/release/ocr server setup --model paddleocr-vl --non-interactive   # builds a venv, weights come from the HF cache
    target/release/ocr server start --model paddleocr-vl

The server's peak footprint is read once from `vmmap --summary` after the run (a peak since the server
started). Exit code 1 (flagged pages) still counts as a result. `production` is only run when named in
`--families`. Other options: `--dpi`, `--extra-args`, `--keep-furniture`, `--git-rev`.
Metrics: CER (whitespace-collapsed, Markdown/HTML table syntax removed, quotes/dashes unified),
token recall, Bates accuracy (page marker), transcript line accuracy (`transcript4up`), pages/s,
peak MB. Same scorer flags for every engine: pleading margin numbers are stripped
(`--strip-line-numbers`, pleading family only) and ground-truth furniture is ignored unless
`--keep-furniture`.

## Seed convention
Tune on seeds 0-9 only. Accept on seeds 10-39, which are never looked at while tuning.

## No handwriting family
Trustworthy ground truth for handwriting cannot be generated, so handwritten annotation pages
(PLAN.md section 7) are checked by hand and are not part of this harness.

## Verification eval (`--verify-json`)
    spike/.venv/bin/python -I eval/verify_eval.py --engine apple [--skip-run] [--use-real-config]

This grades the values and statuses of `--verify-json` against the PDFs' embedded text, on the public set's pages that have it. It scans three times: as users run it, with the embedded text hidden from verify (`OCR_VERIFY_IGNORE_EMBEDDED=1`), and the text layer itself.

It reports:
- field precision, recall and F1;
- word error rate;
- how often `verified` is wrong;
- how often `check` is a false alarm, from the value itself or from a page check;
- how many misread values `check` catches;
- sentences that independent readers agreed on but the embedded text lacks.

The embedded text is not perfect ground truth (old scans carry bad OCR layers), so read the listed examples before trusting a number. Results are in docs/spike.md, "Verification eval".

### With Azure on the flagged pages

`eval/azure_verify_eval.py` simulates the skill's workflow (rely on `verified`, confirm the rest with Azure on `azure_pages`, send what is left to a person) on the committed Mac runs in `eval/verify_bundle/` and raw Azure responses, and grades it against the embedded text. It needs no Mac. The work-machine steps are in `docs/azure-verify-eval.md`.
