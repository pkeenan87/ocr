# `ocr` — Local Document OCR CLI for macOS

Plan v3 · 2026-10-07. Incorporates the Fable review and the Phase 0 measurements in [docs/spike.md](docs/spike.md).

## 1. Goal

`ocr` is a single executable for Apple Silicon Macs running macOS 26+. It turns PDFs and images into Markdown using only local engines:

| Engine flag | Backend | Runs where |
|---|---|---|
| `apple` (default) | Apple Vision `RecognizeDocumentsRequest` (macOS 26+) | In-process, no server |
| `olmocr` | allenai olmOCR 2 (7B, Qwen2.5-VL based) | Local model server |
| `paddleocr-vl` | PaddlePaddle PaddleOCR-VL (0.9B) | Local model server |
| `dots-ocr` | rednote-hilab dots.mocr / dots.ocr | Local model server |

It must handle legal documents that run to hundreds of pages. That means it has to be reliable over long runs, recover from interruptions, keep memory flat and preserve page fidelity.

## 2. Language choice: Rust

| | Rust | Go |
|---|---|---|
| Apple framework access | `objc2-*` crates (PDFKit, CoreGraphics). A `@_cdecl` Swift static lib links through `build.rs` | Needs cgo plus hand-written Objective-C shims. The Swift-only APIs are awkward to reach |
| Single static binary | Yes | Yes |
| CLI ergonomics | `clap` derive | `cobra` |
| Concurrency for page pipelines | `tokio` with bounded channels | goroutines |

**Decision: Rust.** The best Apple engine is `RecognizeDocumentsRequest`, which returns paragraphs, tables and lists. That API is **Swift-only and async**.

**Bridge strategy.** Phase 0 proves the bridge before anything is built on it.
- **Option A, preferred and ✅ proven in Phase 0:** a Swift static library exposing `@_cdecl` functions. Each function blocks on a semaphore around an async `Task`. It is linked by `build.rs`, with Swift runtime rpaths (`/usr/lib/swift`).
- **Option B, fallback:** a Swift helper binary, `ocr-vision`, that speaks JSON over stdin/stdout. It is embedded in `ocr` with `include_bytes!` and extracted to the state dir on first run. The result is still a single shipped file.

## 3. CLI specification

```
ocr server setup  [--model <engine>...] [--all] [--quant 4bit|8bit|bf16] [--non-interactive]
ocr server start  [--model <engine>] [--port 8080]   (default: the only installed model engine)
ocr server stop
ocr server status

ocr scan --input-file <path>  [--output-file <path.md>] [options]
ocr scan --input-dir  <dir>   [--output-dir  <dir>]     [options]

options:
  --model apple|olmocr|paddleocr-vl|dots-ocr   (default: apple)
  --model-version <v>                          (per-engine enum, see §4.2; default = newest)
  --file-type pdf|image|auto                   (default: auto = by extension)
  --recursive                                  (dir mode: walk subfolders, mirror tree in output)
  --pages 1-50,75                              (page range subset)
  --overwrite                                  (discard existing output + checkpoints; otherwise re-runs auto-resume / skip finished files)
  --page-markers / --no-page-markers           (default: on)
  --no-front-matter                            (omit the YAML header, §6)
  --no-currency-fix                            (keep Vision's raw `S1,234.56`; default fixes it to `$1,234.56`)
  --dry-run                                    (print the input → output mapping and exit)
  --use-text-layer auto|never|always           (embedded PDF text instead of OCR; default never for apple, auto for model engines)
  --legal-cleanup                              (opt-in: strip running headers/footers; margin line numbers are always dropped, §4.4)
  --dpi <n>                                    (override render resolution)
  --jobs <n> | --parallel <n>                  (total pages in flight across the whole run; default per engine, see §5.1)
  --quiet | --verbose
```

### Rules

- **Input and output flags.**
  - `--input-file` and `--input-dir` are mutually exclusive, and one of them is required.
  - `--output-file` only works with `--input-file`, and `--output-dir` only works with `--input-dir`.
  - clap enforces these rules.
- **The output is always Markdown.** If `--output-file` doesn't end in `.md`, the tool appends `.md` and prints a note.
- **Default output name** is `<input-stem>.md`. It is used in both modes:
  - Single file with no `--output-file`: `./<stem>.md`, written to the current directory.
  - Directory mode: `<output-dir>/<stem>.md` for each input file.
  - `--output-dir` defaults to `<input-dir>/ocr-output/`. That folder is always skipped when walking the input, including with `--recursive`.
  - Name collisions (`a.pdf` and `a.png` in the same folder) become `a.pdf.md` and `a.png.md`.
- **File types:**
  - `pdf` → `.pdf`.
  - `image` → `.png .jpg .jpeg .tif .tiff .heic .bmp .gif .webp`. Multi-page TIFF counts as a multi-page document.
  - `auto` → both.
  - Files are detected by magic bytes. In directory mode, `--file-type` filters which files are picked up.
- **Setup gate** (decided):
  - No config, `apple` engine: print the warning below, then **carry on**, because Vision needs no download.
  - No config, any model engine (`scan` or `server start`): print the same warning and **exit 2**.
    ```
    ⚠ ocr is not set up yet. Run `ocr server setup` first.
    ```
  - Config exists but the requested model engine isn't installed: the tool warns and exits 2.
    ```
    ⚠ Engine 'olmocr' is not installed. Run `ocr server setup --model olmocr`.
    ```
  - `server stop` and `server status` never hard-fail. They only report.
- **Server gate.** If a model engine is chosen and its server isn't running, the scan exits 3 with `run 'ocr server start --model olmocr'`.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Partial: some pages flagged or failed |
| `2` | Setup or usage error |
| `3` | Server unavailable, or it crashed mid-run. Checkpoints are kept; re-run to resume |

## 4. Architecture

```
ocr (Rust binary)
├── cli/            clap command tree, validation, setup gate
├── config/         ~/.config/ocr/config.toml + state dir ~/Library/Application Support/ocr/
├── input/          discovery, magic-byte file typing, output-path mapping
├── render/         PDF → page images (one PDFDocument per worker), image normalization
├── engines/
│   ├── trait OcrEngine { fn recognize(page) -> PageResult }
│   ├── apple.rs     → Swift bridge (RecognizeDocumentsRequest)
│   ├── olmocr.rs    → HTTP client
│   ├── paddle.rs    → HTTP client + Apple-layout block cropping
│   └── dots.rs      → HTTP client + layout-JSON → Markdown
├── pipeline/       global request semaphore, retries, checkpoints, single-writer manifest
├── markdown/       per-engine normalizers → unified Markdown, page-ordered assembly
└── server/         uv venv, process spawn/stop/restart, PID file, readiness probe
```

### 4.1 Model servers

- **Setup.** `ocr server setup` creates a managed Python environment with `uv` at `~/Library/Application Support/ocr/venv`. It then:
  - installs a pinned `mlx-vlm`;
  - downloads the chosen weights into the HF cache, with progress shown and resumable downloads;
  - checks installed RAM against the needs of each engine and quant (§8.1).
- **Start.** `ocr server start --model X` runs the server bound to localhost only:
  ```
  mlx_vlm.server --host 127.0.0.1 --port <p> --model <repo> --max-num-seqs 1
  ```
  - `--trust-remote-code` is not needed. mlx-vlm 0.7.6 has native model code for all three engines. The `mlx-vlm` version is pinned.
  - The process is detached and writes `server.pid` and `server.json` (engine, repo, port, started_at). Logs go to `server.log`.
- **Readiness check:** `/health` responding **and** a tiny `/v1/chat/completions` probe succeeding. The server loads weights lazily, so `/health` alone is not enough. If the probe fails, the error shows the tail of the log.
- **Stop.** `ocr server stop` sends SIGTERM, waits, then sends SIGKILL and cleans up the PID file. Stale PID files are detected.
- **Status.** `ocr server status` shows the engine, port, uptime and memory.
- **Internal restart.** Used on timeouts (§5.4) and server crashes.
- **One model runs at a time.** Starting a different model asks for confirmation, or accepts `--force`, then stops the current one.
- **API.** The tool talks to the server over the OpenAI-compatible `/v1/chat/completions`.

### 4.2 Engine specifics (re-check repo ids before pinning)

Defaults are the **newest** release of each model. `--model-version` takes a per-engine value from the list below.

| Engine | Default | `--model-version` pins | Prompt / mode | Notes |
|---|---|---|---|---|
| olmOCR 2 | `mlx-community/olmOCR-2-7B-1025-8bit` (`--quant 4bit` available) | — | Upstream `build_no_anchoring_v4_yaml_prompt()`, verbatim. This is olmOCR 2's own default | Longest side 1288 px. About 16 s/page. Output has YAML front-matter, which is parsed for rotation, then stripped. **Drops headers and footers**, so they come from the Apple pass (§4.3) |
| PaddleOCR-VL | **1.6 bf16** (`PaddlePaddle/PaddleOCR-VL-1.6`, 2.4 GB). `--quant 4bit` = `mlx-community/PaddleOCR-VL-1.6-4bit` | `1.5` (`mlx-community/PaddleOCR-VL-1.5-bf16`), `1.0` (`PaddlePaddle/PaddleOCR-VL`) | Full-page `OCR:`. Tables found by Apple Vision are cropped and sent with `Table Recognition:`, which returns OTSL `<fcel>…<nl>` and is converted to GFM | About 2.5 s/page. Full-page `OCR:` was word-perfect on the test page, so per-block cropping is reduced to tables only |
| dots | **dots.mocr** (`mlx-community/dots.mocr-8bit`). Confirmed successor (rednote-hilab, same prompt family). Not `dots.mocr-svg` | `dots.ocr` (original) | Official `prompt_layout_all_en`, verbatim. It returns a JSON **array** of `{bbox, category, text}`, which our converter turns into Markdown. `prompt_ocr` is the cheap mode | About 34 s/page at 200 DPI, dominated by prefill of about 5k image tokens; lower DPI is tuned in Phase 6. Tables come out as HTML, converted to GFM. Formulas come out as LaTeX |
| Apple | Built in | — | `RecognizeDocumentsRequest` with `[en-US]`, auto-detect off and **language correction off**, which caused a `cv`→`cr` error | **About 200 DPI**: on the fixture, 300 and 400 DPI were *less* accurate. Tuned in Phase 2, about 0.15 s/page at 8 jobs. The bridge rebuilds reading order from the bboxes and drops paragraphs that duplicate table or list text. Margin line numbers are filtered by bbox. List markers come from `markerString` |

All model calls use `temperature 0`.

### 4.3 Apple side-pass for model engines

olmOCR drops page headers, footers and Bates numbers, and the other engines label them inconsistently. For every model engine, each page therefore also gets an Apple Vision pass, which is cheap (about 0.15 s per page). It supplies:
- Bates numbers and footer text for the page markers;
- the table regions that PaddleOCR-VL crops;
- the fallback text if the model fails.

### 4.4 Apple layout handling (built in Phase 2)

- **Reading order** is a recursive XY-cut over the block bboxes (`src/layout.rs`). Vision's own paragraph order is not trusted.
  - It applies an x-cut only when each side covers at least 30% of the node's height.
  - Otherwise it cuts at the widest y-gap.
- **Column-aware second pass.**
  - Vision merges lines from different columns that share a baseline, which garbles both columns.
  - When `find_columns` detects at least 2 columns, each column is cropped and re-recognized on its own. The crop runs to the gutter midpoint, with 10% vertical padding.
  - This costs about 2× Vision time on those pages only. Pages are listed in `column_split_pages`.
  - The thresholds were tuned on the fixtures only.

  **Held-out check** (`spike/heldout.py`, scored by `spike/score_heldout.py`). These are layouts the thresholds were never tuned on:

  | Page | Lines found | Order | Column split |
  |---|---|---|---|
  | narrow, unequal columns (62/34, 12 pt gutter) | 21/40 | ok | missed |
  | columns in lower half only | 24/30 | ok | missed |
  | indented block quote | 17/18 | ok | none (correct) |
  | right-aligned signature block | 18/18 | ok | split (harmless false positive) |
  | 4-up deposition transcript | 24/100 | mostly | missed. 300 and 400 DPI don't help |

  The block-based column detector is not robust. **Phase 2b** replaces it.

  **Phase 2b result.** Bitmap XY-cut (`src/regions.rs`), tuned on seeds 0–9 and accepted once on seeds 10–39 (30 seeds, 5 families, clean + scan):

  | | Lines found | Pairs in order |
  |---|---|---|
  | Clean | 97.6% | 99.3% |
  | Scan | 97.7% | 99.3% |

  - Every family is at or above 95.5% of lines found.
  - Single-column goldens are byte-identical.
  - The 300-page benchmark took 28.9 s with a 407 MB peak.
  - The remaining misses are mostly Vision character errors on text in the right place.

    **Transcripts are out of Apple's reach** (docs/spike.md, Phase 2b pre-check). Vision drops deposition line numbers and misreads `Q.`/`A.`, while PaddleOCR-VL got 100/100, even on an uncropped 4-up page. Decided 2026-10-07: Apple flags transcript pages and suggests `--model paddleocr-vl`. **Build order after 2b: server management (4) and the PaddleOCR-VL engine (5, Paddle first), then the long-doc pipeline (3).**
- **Pleading margin line numbers** (1–2 digits in the left 12% of the page) are always dropped from the body. This **changes the v2 decision**, which made it opt-in. Vision reads them only partially and they break up paragraphs. Recovering `page:line` citations properly needs per-line alignment, which is deferred.
- **Apple ignores the text layer by default.** The text-layer path skips all layout handling: margin numbers came out inline, tables were flattened and the Bates number was lost. Feeding text-layer lines through the same layout code, using PDFKit `selectionsByLine()` bounds, is deferred; after that, `auto` can become the Apple default.
- **Scale:** a 300-page mixed PDF took 29 s with a 470 MB peak footprint (vmmap) at `--jobs 8`. Every legal and two-column page was exact.
- **Currency fix.** On Vision pages, `S` before a money pattern (`\bS(\d{1,3}(,\d{3})+|\d+)\.\d{2}\b`) becomes `$`. Fixes are counted per page in `currency_fixes`.

## 5. Making it robust for hundreds of pages

1. **Stream, don't load.** Pages are rendered one at a time into a bounded channel, so memory stays flat whether the document has 10 pages or 2,000.
   - PDFKit is **not** safe to use from several threads on one document, so each render worker opens its own `PDFDocument`.
2. **Born-digital detection.** In `--use-text-layer auto` mode, each page's text layer is quality-checked: character count, ratio of garbage glyphs, and whether a scanned image covers the page. Pages that pass skip the model, and each page's source is recorded.
3. **Per-page checkpoints.**
   - Each finished page goes to `<output>.md.ocr-partial/page-00042.md`.
   - A manifest holds an input fingerprint (size + mtime + SHA-256 of the first and last 1 MB), the engine, model and settings.
   - Re-running the same command resumes automatically. A mismatched fingerprint or settings, or `--overwrite`, starts fresh.
   - The final `.md` is assembled by page number and written atomically (temp file + rename).
4. **Guarding against VLM failure modes:**
   - **Per-page timeout**, set per engine from `max_tokens` divided by the measured decode rate, plus prefill time. dots is already 34 s on a normal page, so one fixed timeout would restart it constantly. The server cannot cancel a request, so on timeout **the server is restarted** and the page retried once. A plain retry would queue behind the stuck request.
   - **Truncation.** `finish_reason == "length"` means the page was cut off. Retry with a higher `max_tokens`, then flag the page.
   - **Repetition loops.** Detect them, retry with a higher repetition penalty, then fall back to Apple Vision and flag the page.
   - **Empty output.** Retry once, then fall back.
   - **Server errors.** A 5xx is retried with backoff.
   - **Server crash or connection refused.** Try one automatic restart. If that fails, keep the checkpoints and exit 3 with "re-run to resume".
5. **Bates numbers and exhibit stamps** are always kept and shown in page markers when found.
   - **Legal cleanups** are **opt-in** (`--legal-cleanup`) because they change citable text. They strip pleading-paper line numbers and repeated running headers and footers.
6. **Page markers.** Each page starts with `<!-- page 42 -->`, or `<!-- page 42 · Bates ABC000123 -->` when a Bates number is found.
7. **Progress.**
   - An overall bar plus bars for the files currently being processed (`indicatif`) show pages per second and ETA.
   - A summary at the end lists pages OCR'd, text-layer pages, fallbacks, flagged pages and elapsed time.

### 5.1 Parallel processing (`--jobs` / `--parallel`)

- **A single setting.** `--jobs N` (alias `--parallel N`) is the **total number of model or Vision requests in flight across the whole run**.
  - In folder mode, pages from several files share one pool. At most about 4 files are open at once.
  - There is no separate files × pages setting.
- **Defaults depend on the engine.** `--jobs 1` forces fully sequential processing.

  | Engine | Default `--jobs` | Why |
  |---|---|---|
  | `apple` | Number of performance cores | Vision runs in-process and scales across cores |
  | model engines | **1** | Measured gains were 0.91–1.42×. Batching changed outputs on 2 of 3 engines, and sequential runs are byte-identical. Rendering and the Apple side-pass still overlap with the model call. `--jobs >1` is opt-in and prints a reproducibility note |
  | text-layer pages | Separate fast path | They never take a slot |

- **The limit counts requests, not pages.** PaddleOCR-VL sends several requests per page.
- **Server concurrency (measured).** mlx-vlm batches requests continuously. The gain is small, and batching makes output depend on what else is in the batch. That is why the default is 1.
- **Thread safety.**
  - Every bridge call creates its own Vision request, with nothing shared between calls. Rust runs these calls on `tokio`'s blocking pool.
  - Each render worker uses its own `PDFDocument`.
- **Order and writes.** Assembly is always by page number. One writer task owns the manifest.
- **Memory.** A page rendered at 300 DPI is about 30 MB, and the queue holds `jobs × 2` pages. `--jobs` is capped by available RAM.

## 6. Output format

```markdown
---
source: Smith_v_Jones_Complaint.pdf
pages: 312
engine: olmocr (mlx-community/olmOCR-2-7B-1025-8bit)
generated: 2026-10-07T19:30:00Z
flagged_pages: [88, 141]
---

<!-- page 1 -->
# IN THE UNITED STATES DISTRICT COURT ...
```

The YAML front-matter can be turned off with `--no-front-matter`.

## 7. Implementation phases

| Phase | Deliverable | Done when |
|---|---|---|
| ✅ **0a. Model spike** (no Rust) | Throwaway `uv` env with mlx-vlm. Run `mlx_vlm.server` with each engine on one test page. Send 2 concurrent requests and time them. Confirm the dots.mocr lineage and its prompts | Each engine returns usable text, or the blocker is documented. Concurrency behavior and the `--jobs` default are recorded in `docs/spike.md` |
| ✅ **0b. Bridge spike** | Install rustup. `cargo new ocr`. Swift `OcrBridge` static lib. A **real `RecognizeDocumentsRequest`** on a test image, called from a Rust worker thread through FFI | Rust prints recognized paragraphs and tables from Vision. If Option A fails, Option B (helper binary) works |
| ✅ **1. CLI + config + setup gate** | Full clap tree, config file, setup (Apple path), gate warnings, exit codes | `ocr scan` before setup: `apple` warns and continues, model engines exit 2. Argument conflicts are rejected |
| ✅ **2. Apple engine end-to-end** | Swift bridge gains page count, per-page text layer, and render-page-*n*-then-Vision (white-filled `CGContext`, no temp PNGs, one document per worker, multi-page TIFF via ImageIO). Vision → Markdown, `--no-front-matter`, file writing. **Reading order:** keep Vision's paragraph order and insert tables/lists before the first paragraph below them; a global top-left sort interleaves columns | A scanned multi-page PDF and a folder of mixed images produce correct `.md` files. On the clean fixture **and** on a degraded-scan fixture (image-only, skew, noise, JPEG), the case number and every dollar amount are exact through the production render path. This re-checks the 200 DPI default. Two-column text comes out column by column |
| ✅ **2b. Robust region detection (contracts)** | Find column regions from the rendered bitmap. Projection profiles at about 100 DPI, with Vision's table boxes masked as solid ink, long thin rule lines removed, and tolerance for scan speckle and skew. Each column region is then cropped for Vision. Flag likely transcript pages (`transcript_pages` in the front matter, plus a stderr hint to use `--model paddleocr-vl`). `heldout.py` becomes seeded and randomized: tune on seeds 0–9 only | On seeds 10–39 (never looked at while tuning), clean and degraded: ≥ 95% of lines found and in order for column layouts. Byte-identical golden output for `legal.pdf`, `legal_scan.pdf` and `two_column.pdf`, apart from `generated:`. 4-up and transcript pages are flagged rather than solved. 300-page benchmark re-run against 29 s / 470 MB |
| ✅ **3. Long-doc pipeline** | `--jobs` pool, ordered assembly, single-writer manifest, auto-resume, atomic writes, progress, page markers | Killing a run mid-way and re-running resumes. Output is identical for `--jobs 1` and `--jobs 8`: byte-for-byte for every engine at default settings, except the `generated:` timestamp line |
| ✅ **4. Server management** | `setup` (venv, weights, RAM check), start/stop/status/restart, readiness probe | Start, stop and status work for each engine. A killed server is detected and restarted |
| ✅ **5. Model engines** | olmOCR (v4 prompt), dots (layout JSON → Markdown), PaddleOCR-VL (full page + table crops with OTSL→GFM), Apple side-pass, timeout/restart, truncation, loop detection, fallback | Each engine passes the eval set |
| **6. Quality** | Eval harness, tuned defaults, opt-in legal cleanup | CER targets met on the eval set |

**Deferred** (after v1):
- re-rendering rotated pages;
- `--json-progress`;
- the `llama-server` backend;
- `--auto-start`;
- Homebrew and notarization.

### Evaluation set (built in Phase 2)

The eval set covers:
- a scanned court filing on pleading paper;
- a born-digital contract;
- a deposition transcript (4-up pages);
- a skewed fax scan;
- a dense table;
- a handwritten annotation page;
- a 500-page production with Bates stamps.

Each run measures CER against hand-checked ground truth for 20 pages, plus throughput and peak memory.

## 8. Risks and open questions

Resolved 2026-10-07:
- Model defaults are the newest versions, pinnable with `--model-version`.
- `<input-stem>.md` is the default output name.
- PaddleOCR-VL uses Apple Vision for page layout.
- `--jobs` / `--parallel` is added.
- The Fable review is incorporated.

- 2026-10-07, **Paddle engine built.** Decisions made in the implementation:
  - `--use-text-layer` defaults to `never` for **all** engines; the text-layer path has no layout handling yet.
  - Model output gets no margin-number filter and no currency fix. Paddle keeps transcript line numbers, and it never makes the `$`→`S` error.
  - The loop retry uses `repetition_penalty 1.15`.
  - Every timeout restarts the server, because the server can't cancel a request.
  - Table crops are full-width bands. On pleading paper the margin numbers therefore land inside `Table Recognition:` crops. Phase 6 checks this on the pleading eval family.
- 2026-10-07, **Phase 3 must fix:** with `--jobs >1` on a model engine, requests queue inside a server started with `--max-num-seqs 1`, and the client's timeout clock includes that queue wait. olmOCR (about 16 s/page) or dots (about 34 s/page) at `--jobs 8` would time out; each timeout restarts the server and kills all in-flight requests, so the failures cascade. Fix: cap in-flight model requests at the server's `--max-num-seqs`, which is in `server.json` argv, or start the timeout clock only when the request reaches the head of the queue.

- 2026-10-08, **olmOCR and dots built** (docs/spike.md, "olmOCR and dots engine spike"). Decisions made for you:
  - **Full page by default for both.** Both read `two_column` and `narrow_unequal` at 100% without cropping. Crops are used only as the first answer to a loop, or to a truncation for dots.
  - **olmOCR scores 0/25 on full-size transcripts** because it drops the line-number margin, so it now prints the hint to use paddleocr-vl. dots scores 25/25 and 100/100.
  - **Limits per engine** (`engines::limits`):

    | Engine | max_tokens | Retry max_tokens | Timeout |
    |---|---|---|---|
    | olmOCR | 4096 | 8000 (upstream's limit) | 220 s |
    | dots | 8192 | — | 190 s |
    | Paddle | 4096 | 8192 | 45 s |

  - **dots never gets `repetition_penalty`**, because it corrupts the JSON. Its ladder is crops, then `prompt_ocr`, then the Apple fallback. A page answered by `prompt_ocr` is not flagged.
  - **Persistent truncation falls back to Apple and is flagged**, rather than keeping the truncated text.
  - **olmOCR ignores `--dpi`**, because images are always 1288 px on the longest side, and prints a note saying so.
  - **Known limit:** on a sideways page the Apple side-pass sees nothing useful. With olmOCR, the Bates number is then missing from that page's marker. A follow-up could run the side-pass on the turned image.
  - **Measured on an idle machine:** olmOCR takes about 16 s/page; dots about 35 s/page at 200 DPI, or 9 s/page at 100 DPI, where all tokens on `legal_scan` were still exact. Phase 6 tunes the dots DPI.

- 2026-10-08, **Phase 3 built** (docs/spike.md, "Phase 3"). Decisions made for you:
  - **Checkpoints** store each page's full result before assembly. Page markers, front matter, `--pages` and `--legal-cleanup` are applied at assembly time, so changing them never invalidates checkpoints. The final file is always assembled from checkpoint records, so resumed output equals clean output by construction.
  - **Failed pages:** a file that ends with failed pages keeps its partial dir. Re-running the same command retries only those pages, and rewrites the `.md` without needing `--overwrite`.
  - **Model requests are capped at the server's `--max-num-seqs` (1).** Each request holds an `flock` on `<state>/server.lock`, and its timeout clock starts only once it has the lock. `--jobs` still sets the Apple side-pass threads. For model engines, "`--jobs 1` = `--jobs 8`" therefore tests side-pass and assembly concurrency, not model batching.
  - **"One PDFDocument per worker"** is already satisfied: every bridge call opens its own document and shares nothing.
  - **No SIGINT message.** Checkpoints persist, and re-running resumes.
  - **Measured:**
    - 300 pages: 28.4 s with a 487 MB peak (baseline 29 s / 470 MB).
    - Resume after a kill at 151 pages: 14.3 s.
    - Live `three_pages.pdf` at `--jobs 1` vs `--jobs 8`: identical for paddleocr-vl (14.3 s), olmocr (47 s) and dots (108 s).

- 2026-10-08, **`--legal-cleanup` built** (commit fc68c84). It runs at assembly time, so turning it on or off never redoes pages. Rules:
  - A line is stripped if it appears in the top 3 or bottom 3 lines of at least 50% of eligible pages (minimum 3 pages), comparing with digits normalized.
  - Transcript pages, table rows, Bates numbers and page markers are never stripped.
  - **Decision made for you:** lines of 20 or more characters match fuzzily, allowing about 1 edit per 20 characters. Apple misreads the same header differently on different pages ("Assel" vs "Asset"), so exact matching left it in. Shorter lines must match exactly.

- 2026-10-08, **Apple stays at 200 DPI (your decision).** On tune seeds 0–9, 150 DPI scored slightly better (token recall 98.2% vs 97.4%, CER 3.10% vs 3.29%). The default stays at 200 so that the golden outputs, and the behaviour they guard, don't change. `--dpi 150` remains available.

- 2026-10-08, **Apple Vision start-up crash: limit of 2 (your decision) plus automatic recovery.**
  - **The crash:** a SIGSEGV in Apple's private TextRecognition framework when several recognitions start at once. Every real crash happened within 0.81 s of launch.
  - **What didn't work:** a first-use gate or a warm-up call (crashes fell only from 4.7% to 3.3% of launches).
  - **What did:** a limit of 2 concurrent Vision requests gave 0 crashes in 100 launches, at about 1.5× slower parallel Apple throughput. You chose that limit.
  - **Safety net:** `ocr scan` also runs as a supervisor that re-runs a crashed child from its checkpoints, up to 3 retries.
  - **Benchmark target:** the Phase 3 300-page figure (28.4 s) will be re-measured under the limit, and the new figure becomes the target.

Still open:

1. **Hardware floor** (vmmap physical footprint): olmOCR 8-bit 10.9 GB, dots 7.3 GB, Paddle bf16 3.8 GB. Setup recommends `--quant 4bit` for olmOCR below 24 GB of RAM, and refuses 8-bit below 16 GB without `--force`.
2. **Text-layer trust.** Bad hidden OCR layers from older scanners exist. The `auto` heuristics need tuning on real samples.
3. **Throughput expectations.** A 300-page scan takes about 1 minute with Apple, 12 minutes with Paddle, 80 minutes with olmOCR and 2.9 hours with dots. The CLI shows an ETA, and the README should state these figures.
4. **Apple accuracy on legal numbers.** On one page, Vision's errors depended on DPI. Phase 2 tunes this on the eval set. A dual-DPI cross-check that flags pages where numbers disagree is a candidate.
5. **Privacy.** Everything is local. Servers bind to localhost only. There is no telemetry. After setup, HF downloads are the only network access, and `HF_HUB_OFFLINE=1` is set at runtime.

## 9. Linux and Windows with a GPU (Phase 7)

**Goal.** `ocr` builds and runs on Linux and Windows machines with a GPU, using the three model engines. macOS is unchanged. This replaces the deferred "`llama-server` backend" item in §7.

### 9.1 What changes off the Mac

| Mac today | Linux / Windows |
|---|---|
| Apple Vision engine (default) | Not available. Default engine is `paddleocr-vl`. The shared types in `vision.rs` (`Block`, `Page`, `DocKind`, `Options`, …) stay portable; only the FFI block and the recognize functions are macOS-only |
| Swift bridge: PDF info, text layer, rendering (PDFKit / ImageIO) | `pdfium-render` (pinned pdfium binary, downloaded by setup) + `image`/`tiff` crates, behind a `Document` trait. The Mac keeps the bridge |
| `mlx-vlm` server in a `uv` venv | `llama-server` from a pinned llama.cpp `b####` release (CUDA, Vulkan or CPU build), downloaded by setup with checksum. **No Python, no uv, no venv** off the Mac |
| `mlx-community/*` weights | GGUF + mmproj repos per engine (§9.3), downloaded from HF with `ureq` |
| RAM check (unified memory) | GPU check + VRAM check (§9.2) |
| Apple side-pass: Bates, footers, table regions, transcript detection, fallback text | Model output plus an olmOCR footer request (§9.5). Bitmap region detection (`regions.rs`) still runs, without Vision's table masks |
| Supervisor (exists only for Vision crashes) | Not compiled |
| `flock`, `process_group`/`killpg`, `ps`, `sysctl`, `vmmap` | `std::fs::File::lock`, `sysinfo`, Windows job object (kill the tree), `available_parallelism` |
| State dir `~/Library/Application Support/ocr` | `dirs::data_dir()/ocr` (`~/.local/share/ocr`, `%APPDATA%\ocr`); `$OCR_STATE_DIR` still wins |

### 9.2 GPU check

`ocr server setup` (and `server start`) probes the hardware:
1. **NVIDIA:** `nvidia-smi --query-gpu=name,memory.total,driver_version` succeeds → CUDA build of llama-server. The driver's CUDA version picks the 12.x or 13.x archive.
2. **Otherwise, a discrete AMD/Intel GPU** → Vulkan build. The probe is a 7a item: `vulkaninfo` is not installed by default, so the likely route is loading the Vulkan loader and listing devices (or asking the downloaded `llama-server --list-devices`).
3. **Integrated GPU only, or none** → the warning below, and the CPU build.

    ⚠ No supported GPU found (NVIDIA with CUDA, or a discrete GPU with Vulkan).
      ocr will run the model on the CPU, which is very slow (minutes per page).

VRAM is then checked against the engine and quant, with the same tiers as the RAM check (§8.1): silent, warn, or refuse without `--force`. Floors come from the 7a spike. `ocr server status` shows the GPU and VRAM in use.

### 9.3 Weights (to confirm in 7a)

| Engine | Default off the Mac | Other quants |
|---|---|---|
| paddleocr-vl 1.6 | `PaddlePaddle/PaddleOCR-VL-1.6-GGUF` (official, f16) | 1.5: `PaddlePaddle/PaddleOCR-VL-1.5-GGUF`. 1.0: community only |
| olmocr | `bartowski/allenai_olmOCR-2-7B-1025-GGUF` Q8_0 + mmproj f16 | Q4_K_M, bf16 |
| dots-ocr | `ggml-org/dots.ocr-GGUF` Q8_0 (first-party, llama.cpp support merged in PR #17575). `--model-version dots.mocr` only if 7a shows a community GGUF works | f16 |

`--quant 4bit|8bit|bf16` maps to Q4_K_M / Q8_0 / f16-or-bf16 where the repo has it.

### 9.4 Phases

| Phase | Deliverable | Done when |
|---|---|---|
| **7a. Backend spike** (no Rust, on the RTX 3090 machine) | Pinned llama-server build with each GGUF. Run the four prompts (`OCR:`, `Table Recognition:`, olmOCR v4 YAML, `prompt_layout_all_en`) through llama-server's chat templates. Test `-np 2` with images. Measure VRAM and s/page per engine and quant. Compare output with the Mac's MLX output and the vLLM reference. Send olmOCR a footer-strip crop: does the Bates number come back? (It is trained to drop footers, and option (b) in §9.5 depends on this.) Settle the Vulkan device probe | `docs/spike.md` records: prompt compatibility (same output shape as MLX), whether parallel image slots work, VRAM floors, speeds. Any engine that fails is documented with its blocker |
| ✅ **7b. Build portability** | `build.rs` and the FFI half of `vision.rs` gated on `target_os = "macos"` (types stay shared). `.cargo/config.toml` rustflags moved under `[target.aarch64-apple-darwin]`. Supervisor macOS-only. `flock` → `File::lock`. Process and memory calls → `sysinfo` / job object. `Engine::Apple` hidden off the Mac | `cargo build` and the unit tests pass on Linux (Mac-only tests gated). `cargo check --target x86_64-pc-windows-msvc` passes. Mac tests unchanged |
| ✅ **7c. Portable document layer** (Mac comparison pending) | `Document` trait over `document_info`, `text_layer`, `render_gray`, `render_png_fit`. pdfium + `image`/`tiff` implementation, including multi-page TIFF and rotation | On the non-table fixtures, `regions.rs` makes the same column and band decisions as on the Mac (renders need not be byte-identical). Table pages differ by design: there are no Vision table masks |
| ✅ **7d-1. llama-server backend** | `Backend` (mlx-vlm / llama-server) in config, `server.json` and checkpoints. `server.json` gets an explicit `slots` field instead of reading `--max-num-seqs` from argv. Checkpoints record a generic runtime version instead of `mlx_vlm`, under the same key so Mac checkpoints stay valid. Setup downloads with `curl` and verifies the sha256 of: the pinned llama.cpp archive (plus cudart for CUDA), pdfium, and the GGUF weights pinned by HF revision. Minimal GPU probe to pick the archive, with an `OCR_LLAMA_VARIANT` override. `-c` per engine from `Limits` x slots | Start, stop, status and restart pass with the fake server on Linux. A real paddleocr-vl server starts on this machine (Vulkan or CPU) |
| **7d-2. Engine layer off the Mac** | A non-Mac `side_pass` (bitmap regions, no blocks). `find_bates` runs on the model's output. Paddle's table-crop branch is off. Failed pages are flagged with no fallback text. The olmOCR footer-strip request (§9.5) | `paddleocr-vl`, `olmocr` and `dots-ocr` each scan `three_pages.pdf` on the Linux GPU machine, with Bates numbers in the page markers |
| ✅ **7e. GPU gate** (CUDA path untested until the 3090) | Hardware probe, build selection, VRAM tiers, the no-GPU warning | This AMD-iGPU machine gets the warning and still runs on CPU. The 3090 machine is silent and picks CUDA |
| **7f. Windows** | `.exe` paths, job-object kill, CI build and unit tests on `windows-latest` (model tests use `tests/fake_server.py`) | A scan runs end-to-end on a Windows + NVIDIA machine |
| **7g. Eval and docs** | Run the eval set (seeds 10–12) on Linux; compare with the Mac and vLLM numbers. README: Linux/Windows requirements, speeds, VRAM table | CER within an agreed margin of the Mac numbers per engine |

### 9.5 Decisions

**Decided 2026-10-08 (your decisions):**
- **Side-pass:** option (b). Bates numbers come from the model output for Paddle and dots. olmOCR gets a footer-strip request, if 7a shows that it returns the Bates number; if not, it falls back to a Paddle `OCR:` on the strip. No table crops, no fallback text.
- **dots default off the Mac:** `dots.ocr` (`ggml-org/dots.ocr-GGUF`).
- **No GPU:** always warn. `paddleocr-vl` carries on, on the CPU. `olmocr` and `dots-ocr` refuse without `--force`.

The options as they were put:

1. **Replacing the Apple side-pass.** On the Mac, Vision supplies every model engine with Bates numbers and footers, the table regions Paddle crops, and fallback text when the model fails. Options:
   - **(a) Accept the loss and document it.** Bates come from the model's own text (`markdown::find_bates` on its output). That works for Paddle and dots, which keep footers. **olmOCR would have no Bates numbers.** Paddle runs full-page `OCR:` only, without table crops. A failed page is flagged with no fallback text.
   - **(b) An extra footer/header strip request through the running model** for olmOCR only. Restores Bates for olmOCR at roughly one short request per page. Everything else as in (a).
   - **(c) A separate CPU OCR pass** (e.g. a pure-Rust or ONNX OCR model) as a stand-in for Vision: Bates, fallback text and transcript hints for all engines. The most faithful, but a second OCR stack to maintain and tune.
   
   Under (a) and (b), **transcript detection is also lost**: it reads Vision's margin-number blocks (`layout::looks_like_transcript`). olmOCR's "use paddleocr-vl" hint on transcripts goes with it, unless it is rebuilt from the bitmap margin.

   Recommendation: **(b)** if 7a shows olmOCR returns Bates from a footer strip. If it doesn't, (b) becomes "run a Paddle `OCR:` on the strip", which means Paddle must be installed alongside olmOCR. Revisit (c) if failed pages without fallback text matter in practice.
2. **dots default off the Mac.** `dots.ocr` (first-party GGUF, supported in llama.cpp) or `dots.mocr` (the Mac default; community GGUF, support unconfirmed). Recommendation: **dots.ocr** unless 7a shows dots.mocr works.
3. **No GPU: warn or refuse?** Recommendation: **warn, then carry on** on CPU for `paddleocr-vl` (0.9B is usable on CPU). For `olmocr` and `dots-ocr`, refuse without `--force`, matching the RAM check.

### 9.6 Testing reality

- This machine (AMD integrated GPU, no NVIDIA driver) can test the build, the document layer, the no-GPU warning and CPU runs. It can't test CUDA.
- The RTX 3090 machine (`docs/linux-reference-eval.md`) is the Linux acceptance target.
- Windows acceptance needs a Windows machine with an NVIDIA GPU. None is known to be available yet.

### 9.7 Progress notes

- 2026-10-08, **7b built.** Decisions made in the implementation:
  - `vision.rs` keeps the shared types and `render_png`. The bridge moved to `vision/apple.rs`. `vision/portable.rs` has stubs that return errors: "not supported yet (7c)" for documents, and "macOS only" for Vision.
  - **Windows stop is TerminateProcess**, via `sysinfo`, instead of a job object. llama-server is a single process, so there is no process tree to kill. The server starts with `CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`, the Windows equivalent of `process_group(0)`. `sysinfo` is a Windows-only dependency. Linux keeps the Unix signal and `ps` code.
  - **Directories:**
    - State: `dirs::data_local_dir()/ocr`. Unchanged on the Mac; `~/.local/share/ocr` on Linux; `%LOCALAPPDATA%\ocr` on Windows.
    - Config: `%APPDATA%\ocr` on Windows; unchanged elsewhere.
  - **Off the Mac:**
    - `server setup` with no `--model` is a usage error.
    - A real model install fails with "not supported on this platform yet (7d)". `OCR_FAKE_INSTALL` still works for tests.
  - **Tests:** the Vision-dependent test files are `#![cfg(target_os = "macos")]`. `tests/cli.rs` uses a fake paddleocr-vl install off the Mac; two of its tests are macOS-only. `tests/server.rs` passes on Linux unchanged.
  - **How it was checked:**
    - Linux: build, clippy, and 118 tests pass.
    - `cargo check --all-targets` is clean for `x86_64-pc-windows-msvc` and `aarch64-apple-darwin`. **`scripts/xcheck.sh`** reproduces this: it caches an official rustc and the std builds in `~/.cache/ocr-xcheck`, because Arch's rustc rejects official std builds. `OCR_SKIP_BRIDGE=1` makes `build.rs` skip the Swift bridge for the macOS check, so Mac tests still need a run on a Mac.
    - **Not run anywhere yet:** the `lock_server` change from `flock` to `File::lock`/`try_lock`. Its test is in `tests/resume.rs`, which is macOS-only. std uses `flock` on both macOS and Linux, so the behaviour should be the same.
  - Off the Mac, the setup gate's hint reads `ocr server setup --model paddleocr-vl`.
  - A `char` range comparison in `markdown.rs` didn't type-check for Windows; it is fixed portably.

- 2026-10-08, **7c built.** Decisions made in the implementation:
  - **Free functions instead of a `Document` trait.** There is one implementation per platform, so `vision.rs` re-exports `vision/document.rs` off the Mac, which keeps 7b's `cfg` pattern.
  - **pdfium is pinned to `chromium/7881`** from bblanchon/pdfium-binaries, matching `pdfium-render` 0.9.4's `pdfium_7881` API. The library is looked for in this order: `$OCR_PDFIUM_LIB`, `<state>/pdfium/`, next to the executable, then the system path. If it isn't found, the error lists every path tried. 7d's setup downloads it into `<state>/pdfium/`. Linux x64 sha256: `1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d`. A development copy is in `~/.cache/ocr-dev/pdfium-7881`.
  - **One lock for every pdfium operation**, held from open to close. `pdfium-render`'s `thread_safe` locking per call still segfaulted with several threads rendering at once (found by the tests). PDF pages therefore render one at a time. That costs little off the Mac, where `--jobs` is 1 for model engines and the Apple side-pass is gone.
  - **Rendering mirrors the bridge:**
    - Sizes are computed in points with `/Rotate` applied (pdfium already reports the rotated size), rounded per axis, and capped at 16384.
    - `max_side` scales by the crop's longest side, renders, crops (origin floored, far edge ceiled, like `CGRect.integral`), then resizes to exactly that size.
    - Rotation is counter-clockwise. Images shrink only when longer than 11 in at the DPI.
    - **Trap:** `pdfium-render`'s *fixed* size renders the page at 72 DPI into a larger bitmap, without scaling. Target sizes must be used instead.
  - **Deliberate differences from the Mac:**
    - Annotations and form fields are not drawn, matching `CGContext.drawPDFPage`. Bates stamps added as annotations would therefore be invisible on both platforms.
    - Alpha is flattened onto white.
    - Grayscale uses Rec.709 weights.
    - The text layer is pdfium's, so spacing and order can differ from PDFKit. `--use-text-layer` defaults to `never`.
    - HEIC is refused with a clear message.
    - TIFF supports 1-bit (including CCITT G4 fax), 8- and 16-bit gray, RGB, RGBA and CMYK. Palette TIFFs are not supported. WhiteIsZero is inverted by the `tiff` crate; it is untested here.
  - **Tests:** `tests/document.rs` runs on both platforms with the same expectations. On a Mac it therefore checks that the bridge and pdfium agree on:
    - sizes, crops, fits and rotations;
    - rotated PDF pages;
    - two-frame G4 TIFF;
    - encrypted PDFs;
    - the text layer;
    - eight concurrent renders.
    
    Off the Mac it also compares pixels with pypdfium2's render (mean difference < 2) and checks the HEIC refusal. New fixtures are in `tests/fixtures/`. Linux: 133 tests pass, clippy is clean, and `scripts/xcheck.sh` is clean.
  - **Acceptance still open:** `examples/region_report` prints render sizes and the regions found without table masks for 14 fixtures. The Linux output is in `docs/region-report-linux.txt`. Run it on the Mac and diff the two to compare column and band decisions.

- 2026-10-08, **7d-1 built; the llama-server half of 7a ran on this machine.** Results are in docs/spike.md, "llama-server spike". Decisions made in the implementation:
  - **Downloads:**
    - Shell out to `curl -fL --retry 3 -C -` and unpack with `tar -xf`. Both ship with current Linux and Windows 10+. This keeps TLS out of the binary, so `ureq` stays localhost-only.
    - Every file is checked against a pinned sha256 before use: release digests for llama.cpp and pdfium, Hugging Face LFS oids pinned by revision for the GGUFs. A `<file>.sha256` stamp skips re-hashing next time.
  - **Layout under `<state>`:**
    - `downloads/` for the archives.
    - `llama.cpp/<build>-<variant>/` for the unpacked builds.
    - `models/<owner>/<repo>/` for the GGUFs.
    - `pdfium/` for the pdfium library.
  - **CUDA variants:** the cudart archive is always downloaded and unpacked into the same folder as `llama-server`. On Linux that folder goes on `LD_LIBRARY_PATH`.
  - **Variant probe:**
    - `OCR_LLAMA_VARIANT` wins if set.
    - An `nvidia-smi` CUDA version of 13 or more picks cuda13; below that, cuda12.
    - Otherwise any Vulkan ICD picks vulkan, otherwise cpu.
    - **7e must refine this.** On integrated graphics the CPU build was 6x faster than Vulkan. A stale `nvidia_icd.json` with no driver also counts as "Vulkan present".
  - **Server arguments:** `--model`, `--mmproj`, `--host 127.0.0.1`, `--port`, `--parallel 1`, `--ctx-size`, plus `--n-gpu-layers 999` unless on the CPU build. `--ctx-size` is (retry `max_tokens` + prompt budget) x slots, which brought CPU memory from 4.5 GB to 2.1 GB. `server.json` records `slots`.
  - **Config and checkpoints:** `[runtime]` gains `llama_cpp` and `llama_variant`. Checkpoints record `llama.cpp b11510 <variant>` under the existing `mlx_vlm` key. Mac configs and checkpoints are unchanged.
  - **Off the Mac:** only versions with GGUF builds are offered (paddle 1.6 and 1.5; dots.ocr). PaddleOCR-VL has a single precision, listed as bf16.
  - **Not in 7d-1:**
    - The VRAM check (7e).
    - `ocr scan` itself: the side-pass still calls Vision (7d-2).
    - Timeouts. `engines::limits` timeouts come from Apple Silicon rates, and the CPU/iGPU path will need longer ones; `OCR_REQUEST_TIMEOUT_SECS` overrides them meanwhile.
  - **The library path** is built from every folder in the install that holds `.so`/`.dll` files, so CUDA's runtime is found wherever its archive unpacks. It is `LD_LIBRARY_PATH` on Linux and `PATH` on Windows.
  - **Checkpoints and builds:** the checkpoint identity includes the build variant, so switching between Vulkan and CPU (or CUDA) re-OCRs pages instead of reusing them. On this machine Vulkan and CPU gave byte-identical text, but CUDA kernels can differ numerically, so the conservative choice stands.
  - **`ocr scan` already runs off the Mac.** On this machine with the CPU build, `legal.pdf` took 7.6 s with every value exact. The Vision side-pass fails quietly and the page goes as one full-page `OCR:` request. **Missing:** the Bates number is in the text but not in the page marker (`<!-- page 1 -->`). Fixing that is the core of 7d-2.

- 2026-10-08, **7e built.** Decisions made in the implementation:
  - **Probe order:**
    1. `OCR_LLAMA_VARIANT`.
    2. `nvidia-smi`: name, memory and CUDA version. 13 or more picks cuda13, below that cuda12.
    3. If a Vulkan driver is installed, setup fetches the Vulkan build (31 MB) and runs `llama-server -v --list-devices`. ggml-vulkan's `uma:` flag separates integrated graphics (`uma: 1`) from GPUs with their own memory. The largest `uma: 0` device gets the Vulkan build.
    4. Integrated-only, or nothing found, gets the CPU build.
    
    This works the same on Linux and Windows, with no sysfs or WMI. It also handles a stale `nvidia_icd.json` with no driver, the case found on this machine.
  - **The warning** keeps your first line. The second line gives the measured speed instead of "minutes per page", which was wrong for PaddleOCR-VL (about 8 s a page on this 16-core CPU). It is shown by `setup` and on every `server start` with the CPU build.
  - **olmocr and dots on the CPU build** are refused by setup without `--force` (your decision). On the CPU build, request timeouts are multiplied by 4, unless `OCR_REQUEST_TIMEOUT_SECS` is set.
  - **Memory check:** need = GGUF model + mmproj + 1.5 GB (KV cache at `--ctx-size` plus compute buffers).
    - Against GPU memory: OK up to 85%, warn up to 100%, refuse above without `--force`.
    - Against RAM on the CPU build: the §8.1 RAM rule.
    - An RTX 3090 fits every engine and quant, olmOCR bf16 (18.1 GB) included.
  - **Status:** `server status` shows the build, the GPU setup found and, with NVIDIA, VRAM used. `[runtime] gpu` records the GPU.
  - **Checked on this machine:** setup with no override found the integrated Radeon and chose CPU, with the warning. olmocr was refused. start/status/stop work. CUDA, discrete Vulkan and Windows are covered only by parser tests and the type-check.
