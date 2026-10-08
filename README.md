# ocr

Local, private OCR of PDFs and images to Markdown on Apple Silicon. Nothing is uploaded anywhere.

`ocr` is a single command-line tool with four engines:

| `--model` | Backend | Runs |
|---|---|---|
| `apple` (default) | Apple Vision `RecognizeDocumentsRequest` | In-process, no download |
| `paddleocr-vl` | PaddleOCR-VL (0.9B) | Local model server |
| `olmocr` | olmOCR 2 (7B) | Local model server |
| `dots-ocr` | dots.mocr (or the original dots.ocr) | Local model server |

It is built for legal documents that run to hundreds of pages: it streams pages, writes per-page checkpoints, resumes after an interruption, keeps Bates numbers in page markers and flags pages it is not sure about.

Status: Phase 6 evaluation is done; results are in [docs/eval.md](docs/eval.md). The fixtures are synthetic.

## Requirements

- Apple Silicon Mac, macOS 26 or later (Apple Vision's document API needs it).
- Xcode command line tools (`xcode-select --install`), for the Swift toolchain.
- Rust, via [rustup](https://rustup.rs), to build.
- For the model engines only: [`uv`](https://docs.astral.sh/uv/). Setup builds a managed Python environment with a pinned `mlx-vlm` (0.7.6) and downloads weights from Hugging Face.

Memory needed by the model engines (peak physical footprint measured with `vmmap`; the server runs one model at a time):

| Engine (default quant) | Weights on disk | Peak memory |
|---|---|---|
| paddleocr-vl 1.6 (bf16) | 1.8 GiB | 3.8-4.0 GB |
| dots-ocr, dots.mocr (8bit) | 4.1 GiB | 6.8 GB at the default 100 DPI (7.6 GB at 200) |
| olmocr (8bit) | 8.8 GiB | 10.9-11.5 GB |

Setup checks installed RAM. For olmOCR 8bit it is silent at 24 GB or more, warns at 16-24 GB and refuses below 16 GB unless you pass `--force`; `--quant 4bit` (about 5.5 GB estimated) is the recommendation on smaller machines. For the other engines it warns when 1.5x the footprint exceeds RAM and refuses when the footprint exceeds 75% of RAM. Apple Vision needs no download; a 500-page production peaked at about 0.5 GB.

## Build and install

    cargo build --release

The binary is `target/release/ocr`. Copy it onto your PATH if you like:

    cp target/release/ocr /usr/local/bin/

## First run and setup

Apple Vision works immediately. `ocr scan` before any setup prints a warning and carries on:

    ⚠ ocr is not set up yet. Run `ocr server setup` first.

To write the config explicitly (Apple needs no download):

    ocr server setup

To install a model engine:

    ocr server setup --model paddleocr-vl
    ocr server setup --model paddleocr-vl --quant 4bit --model-version 1.5
    ocr server setup --model olmocr --quant 4bit
    ocr server setup --all

`--model` is repeatable; other setup flags are `--quant 4bit|8bit|bf16`, `--model-version`, `--non-interactive` and `--force` (skip the RAM check). Valid versions: paddleocr-vl `1.6` (default), `1.5`, `1.0`; dots-ocr `dots.mocr` (default), `dots.ocr`. olmOCR has no selectable version. Not every quant exists for every release; the error message lists those that do. Defaults: paddleocr-vl bf16, olmocr and dots-ocr 8bit.

For a model engine, a missing or incomplete setup is an error (exit 2), not a warning:

    ⚠ Engine 'olmocr' is not installed. Run `ocr server setup --model olmocr`.

Model engines also need a running server:

    ocr server start --model paddleocr-vl     # --port 8080 by default; --force replaces a running different model
    ocr server status                         # engine, port, uptime, memory
    ocr server stop

`server start` defaults to the only installed model engine. Only one model runs at a time. The server binds to 127.0.0.1 and `start` waits until a real test request succeeds. If you `scan` with a model engine and no matching server is running, the scan exits 3 with `model server not running; run 'ocr server start --model <engine>'`.

## Scanning

Usage (`ocr scan --help`):

    ocr scan [OPTIONS]

    --input-file <PATH>        Single input file
    --input-dir <DIR>          Input directory
    --output-file <PATH.md>    Output Markdown file (default ./<stem>.md)
    --output-dir <DIR>         Output directory (default <input-dir>/ocr-output)
    --model <MODEL>            [default: apple] [apple, olmocr, paddleocr-vl, dots-ocr]
    --model-version <V>        Pin an older model release (paddleocr-vl: 1.6|1.5|1.0, dots-ocr: dots.mocr|dots.ocr)
    --file-type <FILE_TYPE>    [default: auto] [pdf, image, auto]
    --recursive                Walk subfolders and mirror the tree in the output
    --pages <PAGES>            Page subset, e.g. 1-50,75
    --overwrite                Discard existing output and checkpoints
    --page-markers             Start each page with an HTML comment marker (default)
    --no-page-markers          Omit per-page markers
    --no-currency-fix          Keep `S` where Vision misread `$` in amounts such as S22,375.50
    --no-front-matter          Omit the YAML header
    --use-text-layer <MODE>    auto, never, always (default: never)
    --legal-cleanup            Strip running headers and footers repeated across pages (opt-in)
    --dpi <N>                  Render resolution, 72-600 (default 200; paddleocr-vl 150; dots-ocr 100)
    --jobs <N>                 Total pages in flight across the whole run [alias: --parallel]
    --quiet / --verbose
    --dry-run                  Print the planned input -> output mapping and exit

`--input-file` and `--input-dir` are mutually exclusive and one is required; `--output-file` goes only with `--input-file`, `--output-dir` only with `--input-dir`.

Examples:

    ocr scan --input-file complaint.pdf                          # writes ./complaint.md with Apple Vision
    ocr scan --input-file scan.pdf --output-file out/scan.md
    ocr scan --input-dir ./case                                  # writes ./case/ocr-output/<stem>.md
    ocr scan --input-dir ./case --recursive --file-type pdf      # walk subfolders, PDFs only
    ocr scan --input-dir ./case --file-type image --dry-run      # show the mapping, do nothing
    ocr scan --input-file depo.pdf --model paddleocr-vl          # deposition transcript
    ocr scan --input-file big.pdf --pages 1-50,75
    ocr scan --input-file big.pdf --jobs 4                       # Apple: 4 pages at once
    ocr scan --input-file big.pdf --overwrite                    # discard output and checkpoints
    ocr scan --input-file big.pdf --legal-cleanup                # strip repeated headers/footers
    ocr scan --input-file big.pdf --dpi 150
    ocr scan --input-file big.pdf --use-text-layer auto          # use embedded text where it is good
    ocr scan --input-file big.pdf --no-page-markers --no-front-matter --no-currency-fix

Notes on the options:

- **`--model-version`** pins an older model release; it applies to `paddleocr-vl` and `dots-ocr` only.
- **`--jobs` / `--parallel`** is the total number of pages in flight. The default is the number of performance cores for Apple and 1 for model engines. A model server handles one request at a time (`--max-num-seqs 1`), so `--jobs` above 1 on a model engine only overlaps rendering, the Apple side-pass and assembly, and prints a note. Output is the same for `--jobs 1` and `--jobs 8`.
- **`--use-text-layer`** defaults to `never` for every engine. The text-layer path has no layout handling: margin line numbers come out inline, tables are flattened and the Bates number is lost. Use `auto` or `always` only for clean born-digital PDFs where speed matters.
- **`--dpi`** has a per-engine default tuned in Phase 6: Apple 200 (150 scored better on the synthetic fixtures, mostly on fax scans, but 200 is kept so the goldens stay stable; try `--dpi 150` on poor scans), paddleocr-vl 150, dots-ocr 100 (as accurate as 200 and about 3x faster). olmOCR ignores `--dpi` (images are always 1288 px on the longest side) and says so.
- **`--legal-cleanup`** is opt-in because it changes citable text. A line is stripped when it sits in the top or bottom three lines of at least half the pages (minimum 3 pages); lines of 20 or more characters match fuzzily. Transcript pages, table rows, Bates numbers and page markers are never stripped. It runs at assembly time, so toggling it never redoes OCR.
- **`--no-currency-fix`**: Vision sometimes reads `$` as `S`. By default `S22,375.50` becomes `$22,375.50` on Apple pages (counted per page in the front matter). Model-engine output is not touched.
- **`--file-type`**: pdf is `.pdf`; image is `.png .jpg .jpeg .tif .tiff .heic .bmp .gif .webp` (multi-page TIFF is a multi-page document). In directory mode it filters which files are picked up.

### Output naming

- Single file: `./<stem>.md` in the current directory unless `--output-file` is given. A name not ending in `.md` gets `.md` appended, with a note.
- Directory mode: `<output-dir>/<stem>.md`, with `--output-dir` defaulting to `<input-dir>/ocr-output/`. That folder is always skipped when walking the input. With `--recursive` the subfolder tree is mirrored.
- Collisions (`a.pdf` and `a.png` in one folder) become `a.pdf.md` and `a.png.md`.

### Resume

Each finished page is checkpointed to `<out>.md.ocr-partial/` next to the output file. If a run is interrupted (Ctrl-C, kill, crash, server loss), run the same command again and it resumes where it stopped; finished files in a directory run are skipped. A changed input, engine or setting (or `--overwrite`) starts fresh. The final `.md` is assembled from the checkpoints and written atomically. Page markers, front matter, `--pages` and `--legal-cleanup` are applied at assembly, so changing them does not invalidate checkpoints. A file that ends with failed pages keeps its partial directory; re-running retries only those pages.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Partial: some pages were flagged or failed |
| 2 | Setup or usage error (including a model engine that is not installed) |
| 3 | Model server unavailable, or it crashed mid-run. Checkpoints are kept; re-run to resume |

## Output format

    ---
    source: Smith_v_Jones_Complaint.pdf
    pages: 312
    engine: olmocr (mlx-community/olmOCR-2-7B-1025-8bit)
    generated: 2026-10-07T19:30:00Z
    text_layer_pages: []
    column_split_pages: [12, 13]
    transcript_pages: []
    table_pages: [40]
    fallback_pages: []
    flagged_pages: [88, 141]
    ---

    <!-- page 1 · Bates SMITH000123 -->
    # IN THE UNITED STATES DISTRICT COURT ...

Front matter fields (`--no-front-matter` omits the header):

- `source`: input file name. `pages`: page count.
- `engine`: engine, with the model repo for model engines.
- `generated`: UTC timestamp. The only line that differs between otherwise identical runs.
- `text_layer_pages`: pages taken from the embedded PDF text instead of OCR.
- `column_split_pages`: pages where multi-column regions were detected and recognized column by column.
- `transcript_pages`: pages that look like deposition transcripts (numbered Q./A. lines).
- `table_pages` and `fallback_pages` (model engines): pages with a table recognized separately, and pages where the model failed and Apple Vision text was used instead.
- `currency_fixes` (Apple engine, instead of the two above): `{page: count}` of `S` to `$` corrections.
- `legal_cleanup` (with `--legal-cleanup`): number of stripped lines.
- `flagged_pages`: pages to check by hand: failed pages, fallbacks that lost quality, transcripts Apple cannot read, and so on.

Every page starts with a marker, `<!-- page 42 -->`, or with the Bates number when one is found at the bottom of the page: `<!-- page 3 · Bates ABC0000123 -->`. A page that failed outright appears as `<!-- page N: OCR failed: ... -->`. Page numbers are 1-based and always match the source document, also with `--pages`. When a model fails on a page (timeout, runaway repetition, empty or truncated output) it is retried and then falls back to Apple Vision text; that page is listed in `fallback_pages` and `flagged_pages`.

## Which engine?

| Engine | Best for | Speed (measured) | 300-page estimate | Memory (server peak) |
|---|---|---|---|---|
| apple | General documents, contracts, pleadings, scans, bulk runs. Not transcripts | about 16 pages/s on a 500-page production (before the Vision concurrency cap, see below) | well under a minute | `ocr` about 0.5 GB |
| paddleocr-vl | The fastest model engine; text-heavy documents, tables | about 0.19 pages/s (5.3 s/page at 150 DPI) | about 27 minutes | 4.0 GB |
| olmocr | Clean prose and tables in ordinary documents. Not transcripts | about 0.06 pages/s (about 16 s/page, from the spike) | about 80 minutes | 11-11.5 GB (5.5 GB at 4bit, estimated) |
| dots-ocr | **Deposition transcripts (best, but verify line numbers)** and layout-heavy pages | about 0.086 pages/s (11.6 s/page at 100 DPI) | about 58 minutes | 6.8 GB |

Throughput comes from [docs/eval.md](docs/eval.md) on a 32 GB M-series Mac: Apple on a quiet machine, Paddle on a 500-page production, dots on 60 pages, olmOCR from the spike (my eval runs of it were contended). Apple throughput after the Vision concurrency cap (2 at a time, a crash fix) is slower, about 1.5x; the re-measured figure is pending. Accuracy on the synthetic eval set (accept seeds; token recall is the primary metric):

| Engine | Token recall | CER | Bates | Notes |
|---|---|---|---|---|
| apple | 97.4% (no transcripts) | 2.3% | 93/105 pages on pleadings | fax scans 88.5% at 200 DPI, 96.3% at 150 |
| paddleocr-vl | 99.6% (no transcripts) | 3.3% (pleading CER inflated by margin numbers) | 100% | transcripts 81.5% |
| dots-ocr | 100% (no transcripts and 99.8% on a 60-page production) | 0.6% | 100% | transcripts 100% recall, 68% lines exact |
| olmocr | 99.3% (no transcripts) | 0.0% | 10 of 11 pages | transcripts 96.9% recall, drops line numbers |

Guidance:

- **Start with Apple.** It is fast and light, with exact case numbers and amounts on the test fixtures, and does not need a server.
- **Deposition transcripts: `dots-ocr`, and check the line numbers.** It has the best recall and line accuracy on the transcript fixtures (100% / 68%), but every engine gets line numbers wrong after a wrapped line (transcripts number each physical line). Verify page:line citations against the PDF. Apple flags transcript pages; its hint still names `paddleocr-vl`, which reads the words but renumbers lines after wraps (25% of lines exact), so treat that hint with care.
- **olmOCR: not for transcripts.** It drops the margin line numbers and prints a hint. It also drops headers, footers and Bates numbers; those come from an Apple Vision side-pass that every model engine runs.
- **Pleadings with tables on `paddleocr-vl`:** its table crops pull the margin line numbers in as an extra column. Use another engine for pleading-paper tables.

## Limitations

- Apple Vision cannot read deposition transcripts reliably: it drops the line numbers and misreads `Q.` and `A.`. Such pages are flagged. No engine gets transcript line numbers reliably after wrapped lines; verify page:line citations.
- The text-layer path (`--use-text-layer auto|always`) has no layout handling, which is why it is off by default.
- Pleading margin line numbers (1-2 digits in the left margin) are dropped by Apple Vision, dots and olmOCR but kept by `paddleocr-vl`, where they can be glued to paragraph numbers and land in table crops on pleading paper. Recovering `page:line` citations is not implemented.
- With olmOCR, a rotated page loses its Bates number in the marker, because the Apple side-pass cannot read a sideways page.
- Column detection was tuned on synthetic fixtures; it found about 97.6% of lines on held-out synthetic layouts, and unusual layouts can still be mis-ordered.
- Fixtures and evaluation data are synthetic, not real filings. Apple Bates accuracy on pleadings was 88.6% on the accept seeds.
- Apple Vision crashed intermittently at process start in the evaluation binary (an Apple framework init race); the current build caps Vision at 2 concurrent requests and re-runs a crashed child from its checkpoints.
- Handwriting is not evaluated; there is no trustworthy ground truth for it.
- Model engines are slower than they look: a 300-page scan takes minutes to hours depending on the engine, so use the progress bar ETA and rely on resume.

## Privacy

Everything runs on your machine. Model servers bind to 127.0.0.1 only, there is no telemetry, and the only network access is the Hugging Face and Python package downloads during `ocr server setup`. At run time the servers are started with `HF_HUB_OFFLINE=1`.

## Development

    cargo test --release                       # unit and integration tests
    OCR_REAL_MODELS=1 cargo test --release     # also run tests against real model servers (slow, needs setup)

Layout: `src/` (Rust CLI and pipeline), `bridge/` (Swift static library for Apple Vision and PDF rendering), `tests/`, `examples/` (probes), `spike/` (Phase 0 to 2b experiments and fixtures), `eval/` (Phase 6 evaluation harness, see [eval/README.md](eval/README.md)).

Documents:

- [PLAN.md](PLAN.md): the specification and all decisions.
- [docs/spike.md](docs/spike.md): measurements to date.
- [docs/eval.md](docs/eval.md): evaluation method, per-engine results, targets and known gaps.
