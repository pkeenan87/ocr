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

## Candidate models (raw harness)

`eval/candidates.json` pins the local-model lineup for evaluation: the three shipped engines as `*-raw` baselines (paddle-raw, olmocr-raw, dots-ocr-raw) and the candidates dots-mocr, glm-ocr, ovisocr3 and lighton-4b. Each entry fixes a repo revision, sha256-verified GGUF (model + mmproj), the model's upstream prompt and its render/max-tokens settings, so every request is one full page at temperature 0 — no crops, retries, Bates corners or stamp restore.

To run one: `python3 eval/candidate_serve.py <name> --gpu 1` in one terminal (fetches the pinned files and starts `llama-server` on port 8095); then `cargo build --release --example verify_md`, `python3 eval/candidate_run.py <name>` (add `--overwrite` to redo), and `python3 eval/holdout/score.py --engine cand-<name> --json eval/out/holdout/cand-<name>.json`.

For the synthetic sets, `eval/vllm_reference.py` takes `--candidate <name>` (instead of `--engine`) and scores the served model against the fixtures: `python3 eval/vllm_reference.py --candidate <name> --set eval/out/seed10 ... --out eval/out/candidates --port 8095`. `spike/.venv/bin/python eval/candidate_report.py` prints the results rows (filing-stamp dockets and dates split out). The full plan, gates and results table are in [docs/model-lineup-eval.md](../docs/model-lineup-eval.md).

## Seed convention
Tune on seeds 0-9 only. Accept on seeds 10-39, which are never looked at while tuning.

## No handwriting family
Trustworthy ground truth for handwriting cannot be generated, so handwritten annotation pages
(PLAN.md section 7) are checked by hand and are not part of this harness.

## Verification eval (`--verify-json`)
    spike/.venv/bin/python -I eval/verify_eval.py --engine apple [--skip-run] [--use-real-config]

This grades the values and statuses of `--verify-json` against the PDFs' embedded text, on the public set's pages that have it (not on pages whose text is an invisible OCR layer). It scans four times: as users run it; read without the embedded text (`--use-text-layer never`, so nothing is corrected from it) and verified with it, which grades; the same reading with the embedded text hidden from verify too (`OCR_VERIFY_IGNORE_EMBEDDED=1`), which is the blind run; and the text layer itself.

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

### Strict and normalized error rates

`eval/score.py` reports, per file, CER as before (NFKC, quotes and dashes unified, markup removed, ground-truth furniture removed) and also:
- `cer_strict`: the same text kept as written (NFC only, curly quotes and dashes as they are);
- `wer` and `wer_strict`;
- the character edits by kind: `subs`, `dels` (reference text the output lacks: dropped words) and `ins` (extra output).

`run.py` aggregates them, and `agg.py` prints them for result files that have them.

`eval/verify_eval.py` adds:
- a strict word error rate against the embedded text (case, punctuation, decimal points and signs kept);
- the reference words deleted;
- the pages with a run of 12 or more reference words that appear nowhere on the OCR page, which usually means a dropped line or paragraph. Text that the OCR has elsewhere on the page counts as moved, not dropped. Misreads can still put a page on this list, so it is a list of pages to look at, not a count of drops.

## MiniCPM-OCR 1.0 and MiniCPM-OCR Jev

`eval/minicpm/compare.py` is an opt-in comparison against `paddleocr-vl` (pass `--model` explicitly; the binary default is unchanged). MiniCPM-OCR Jev here is OpenBMB's local region scorer, not the TypeSafe judge in [JUDGE.md](JUDGE.md). Corrections are the Export JSON from `eval/holdout/review_template.html`: `contract.pdf` pairs with `contract.json` whose `pages[].doc` is `contract`. Put real scans in `spike/heldout-local/` (gitignored). Results go to `eval/out/minicpm/`. Setup, GPUs, and the two-pass commands for a 2×16GB Linux box are in the README section "MiniCPM-OCR comparison". `python3 eval/minicpm/selftest.py` runs the harness against a fake localhost server and a synthetic contract.
