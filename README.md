# ocr

Local, private OCR of PDFs and images to Markdown on Apple Silicon, and on Linux and Windows machines with a GPU (see [Linux and Windows](#linux-and-windows)). Nothing is uploaded anywhere.

`ocr` is a single command-line tool with four engines:

| `--model` | Backend | Runs |
|---|---|---|
| `apple` (default) | Apple Vision `RecognizeDocumentsRequest` | In-process, no download |
| `paddleocr-vl` | PaddleOCR-VL (0.9B) | Local model server |
| `olmocr` | olmOCR 2 (7B) | Local model server |
| `dots-ocr` | dots.mocr (or the original dots.ocr) | Local model server |

It is built for legal documents that run to hundreds of pages: it streams pages, writes per-page checkpoints, resumes after an interruption, keeps Bates numbers in page markers and flags pages it is not sure about.

Status: Phase 6 (quality evaluation) is in progress. Numbers marked `TODO(eval)` are pending the final evaluation.

## Requirements

- Apple Silicon Mac, macOS 26 or later (Apple Vision's document API needs it).
- Xcode command line tools (`xcode-select --install`), for the Swift toolchain.
- Rust, via [rustup](https://rustup.rs), to build.
- For the model engines only: [`uv`](https://docs.astral.sh/uv/). Setup builds a managed Python environment with a pinned `mlx-vlm` (0.7.6) and downloads weights from Hugging Face.

Memory needed by the model engines (peak physical footprint measured with `vmmap`; the server runs one model at a time):

| Engine (default quant) | Weights on disk | Peak memory |
|---|---|---|
| paddleocr-vl 1.6 (bf16) | 1.8 GiB | 3.8 GB |
| dots-ocr, dots.mocr (8bit) | 4.1 GiB | 7.3 GB |
| olmocr (8bit) | 8.8 GiB | 10.9 GB |

Setup checks installed RAM. For olmOCR 8bit it is silent at 24 GB or more, warns at 16-24 GB and refuses below 16 GB unless you pass `--force`; `--quant 4bit` (about 5.5 GB estimated) is the recommendation on smaller machines. For the other engines it warns when 1.5x the footprint exceeds RAM and refuses when the footprint exceeds 75% of RAM. Apple Vision needs no download; a 300-page scan peaks at about 0.5 GB.

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

## Linux and Windows

Status: Phase 7 (PLAN.md §9). Linux was tested on a Ryzen 9950X, which has no discrete GPU, with the CPU build. The RTX 3090 (CUDA) run and a native Windows run are still to come. Windows builds and tests run in CI.

Off the Mac:
- There is no Apple Vision, so `apple` is not offered and **`paddleocr-vl` is the default engine**.
- The models run in llama.cpp's `llama-server` (release b11510) instead of mlx-vlm, from GGUF weights. No Python is needed.
- PDFs are rendered with pdfium (`chromium/7881`).

**Requirements:**
- Linux x86-64 or Windows 10 (version 1803 or later) x86-64. Setup uses the `curl` and `tar` that ship with both.
- Rust, to build.
- A GPU is strongly recommended: NVIDIA (CUDA), or any GPU with its own memory through Vulkan.

**Setup** probes the hardware, downloads the matching llama.cpp build, pdfium and the weights, and checks every file against a pinned sha256:

    cargo build --release
    ocr server setup --model paddleocr-vl
    ocr server start --model paddleocr-vl
    ocr scan --input-file filing.pdf

**How the build is chosen:**

| Hardware | llama.cpp build |
|---|---|
| NVIDIA driver (`nvidia-smi`) | CUDA 12 or 13, whichever the driver supports |
| A Vulkan GPU with its own memory (AMD, Intel Arc, ...) | Vulkan |
| Integrated graphics only, or no GPU | CPU, with a warning |

Integrated graphics get the CPU build because it was 6x faster than Vulkan on a Ryzen's built-in Radeon. With the CPU build:
- `olmocr` and `dots-ocr` need `--force` at setup.
- `paddleocr-vl` runs at about 8 s a page on a 16-core CPU.
- Request timeouts are four times as long.

Set `OCR_LLAMA_VARIANT=cuda12|cuda13|vulkan|cpu` to override the choice. If a CUDA build won't start, try `OCR_LLAMA_VARIANT=vulkan ocr server setup --model <engine>`; NVIDIA cards run Vulkan too.

**Memory:** need is the GGUF files plus about 1.5 GB. Setup warns above 85% of GPU memory and refuses above 100% unless `--force`.

| Engine (`--quant`) | Weights | Needs about |
|---|---|---|
| paddleocr-vl 1.6 (bf16, the only GGUF) | 1.8 GB | 3.3 GB |
| dots-ocr, dots.ocr (8bit = Q8_0) | 3.2 GB | 4.7 GB |
| olmocr (4bit = Q4_K_M) | 6.0 GB | 7.5 GB |
| olmocr (8bit = Q8_0, default) | 9.5 GB | 10.9 GB |
| olmocr (bf16) | 16.6 GB | 18.1 GB |

Measured: PaddleOCR-VL on the CPU build uses 2.1 GB resident. TODO(eval): measure GPU memory on the RTX 3090.

**Differences from the Mac:**
- dots defaults to `dots.ocr`, because llama.cpp's support for dots.mocr is unconfirmed. Only PaddleOCR-VL 1.6 and 1.5 are offered.
- Without the Apple Vision side-pass:
  - Bates numbers come from the model's own text.
  - olmOCR, which drops footers, gets two extra small requests per page, for the bottom-right and bottom-left corners. A Bates number centred under the footer is not found.
  - There are no table crops and no transcript detection.
  - A page the model fails is recorded as failed, and a re-run retries it, instead of falling back to Apple Vision text.
- Column detection still works: it runs on the rendered bitmap.
- HEIC images are refused; convert them to PNG or JPEG.

Files live in `~/.local/share/ocr` (Linux) or `%LOCALAPPDATA%\ocr` (Windows). Config lives in `~/.config/ocr` or `%APPDATA%\ocr`.

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
    --dpi <N>                  Render resolution, 72-600 (default 200)
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
- **`--dpi`** defaults to 200, which was the most accurate for Apple Vision (higher is not better; 150 scored marginally better on the tuning seeds but 200 is kept). olmOCR ignores `--dpi` (images are always 1288 px on the longest side) and says so. For dots, lower DPI is much faster (about 34 s/page at 200 DPI, 17 s at 150, 9 s at 100 on one test page).
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

| Engine | Best for | Speed | 300-page estimate | Memory |
|---|---|---|---|---|
| apple | General documents, contracts, pleadings, scans, bulk runs | about 10 pages/s (8 jobs) | about 30 s (measured, 28.4 s) | about 0.5 GB |
| paddleocr-vl | Deposition transcripts; best speed/quality among model engines | about 0.4 pages/s (about 2.5 s/page) | about 12 minutes | 3.8 GB |
| olmocr | Clean prose and tables in ordinary documents; not transcripts | about 0.06 pages/s (about 16 s/page) | about 80 minutes | 10.9 GB (5.5 GB at 4bit, estimated) |
| dots-ocr | Transcripts and layout-heavy pages when time does not matter | about 0.03 pages/s (about 34 s/page at 200 DPI) | about 2.9 hours | 7.3 GB |

The per-page figures are from docs/spike.md on a nearly idle 32 GB M-series Mac. Final evaluation results (accuracy, throughput, memory) are pending: TODO(eval) accuracy per engine and family (CER, token recall, Bates accuracy, transcript line accuracy); TODO(eval) measured pages/s per model engine on the eval set; TODO(eval) final 300-page estimates; TODO(eval) tuned dots DPI and its speed; TODO(eval) peak memory per engine from the eval run.

Guidance:

- **Start with Apple.** It is fast and light, with exact case numbers and amounts on the test fixtures, and does not need a server.
- **Deposition transcripts: `paddleocr-vl`.** It keeps the line numbers and Q./A. markers (100/100 on the test transcripts). Apple Vision drops them, so Apple flags such pages and suggests `--model paddleocr-vl`.
- **olmOCR: not for transcripts.** It drops the margin line numbers (0/25 exact on a full-size transcript) and prints a hint to use paddleocr-vl. It also drops headers, footers and Bates numbers; those come from an Apple Vision side-pass that every model engine runs.
- **dots: also reads transcripts exactly (25/25 and 100/100) but is slow.** Consider `--dpi 150` or lower to speed it up.

## Limitations

- Apple Vision cannot read deposition transcripts reliably: it drops the line numbers and misreads `Q.` and `A.`. Such pages are flagged, with a hint to use `--model paddleocr-vl`.
- The text-layer path (`--use-text-layer auto|always`) has no layout handling, which is why it is off by default.
- Pleading margin line numbers (1-2 digits in the left margin) are dropped by Apple Vision but kept by the model engines (and they can land inside table crops on pleading paper). Recovering `page:line` citations is not implemented.
- With olmOCR, a rotated page loses its Bates number in the marker, because the Apple side-pass cannot read a sideways page.
- Column detection was tuned on synthetic fixtures; it found about 97.6% of lines on held-out synthetic layouts, and unusual layouts can still be mis-ordered.
- Fixtures and evaluation data are synthetic, not real filings.
- Handwriting is not evaluated; there is no trustworthy ground truth for it.
- Model engines are slower than they look: a 300-page scan takes minutes to hours depending on the engine, so use the progress bar ETA and rely on resume.

## Privacy

Everything runs on your machine. Model servers bind to 127.0.0.1 only, and there is no telemetry. The only network access is the downloads during `ocr server setup`:
- macOS: Hugging Face and Python packages. At run time the servers start with `HF_HUB_OFFLINE=1`.
- Linux and Windows: the pinned llama.cpp and pdfium releases from GitHub, and the GGUF weights from Hugging Face, all checked by sha256.

## Development

    cargo test --release                       # unit and integration tests
    OCR_REAL_MODELS=1 cargo test --release     # also run tests against real model servers (slow, needs setup)
    scripts/xcheck.sh                          # from Linux: type-check the Windows and macOS targets

Off macOS, tests that render documents need pdfium. It is found at `$OCR_PDFIUM_LIB`, `<state>/pdfium/` (setup installs it there), next to the binary, or on the system library path. CI (`.github/workflows/ci.yml`) runs fmt, clippy and the tests on Linux and Windows. The macOS tests need a Mac with macOS 26.

Layout: `src/` (Rust CLI and pipeline), `bridge/` (Swift static library for Apple Vision and PDF rendering), `tests/`, `examples/` (probes), `spike/` (Phase 0 to 2b experiments and fixtures), `eval/` (Phase 6 evaluation harness, see [eval/README.md](eval/README.md)).

Documents:

- [PLAN.md](PLAN.md): the specification and all decisions.
- [docs/spike.md](docs/spike.md): measurements to date.
- docs/eval.md: evaluation results (coming).
