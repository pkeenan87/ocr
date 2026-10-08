# `ocr` — Local Document OCR CLI for macOS

Plan v3 · 2026-10-07 (updated 2026-10-08, Phase 6 done). Incorporates the Fable review and the Phase 0 measurements in [docs/spike.md](docs/spike.md).

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

    **Transcripts are out of Apple's reach** (docs/spike.md, Phase 2b pre-check). Vision drops deposition line numbers and misreads `Q.`/`A.`, while PaddleOCR-VL got 100/100, even on an uncropped 4-up page. Decided 2026-10-07: Apple flags transcript pages and suggested `--model paddleocr-vl`. **Superseded 2026-10-08:** line numbers are re-anchored to page geometry for every engine, the hints point to `line_numbers.unverified`, and dots-ocr is the recommended engine for transcripts (§8). **Build order after 2b: server management (4) and the PaddleOCR-VL engine (5, Paddle first), then the long-doc pipeline (3).**
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
| ✅ **2b. Robust region detection (contracts)** | Find column regions from the rendered bitmap. Projection profiles at about 100 DPI, with Vision's table boxes masked as solid ink, long thin rule lines removed, and tolerance for scan speckle and skew. Each column region is then cropped for Vision. Flag likely transcript pages (`transcript_pages` in the front matter, plus a stderr hint; the hint originally pointed to `--model paddleocr-vl` and now points to `line_numbers.unverified` and recommends dots-ocr, see §8). `heldout.py` becomes seeded and randomized: tune on seeds 0–9 only | On seeds 10–39 (never looked at while tuning), clean and degraded: ≥ 95% of lines found and in order for column layouts. Byte-identical golden output for `legal.pdf`, `legal_scan.pdf` and `two_column.pdf`, apart from `generated:`. 4-up and transcript pages are flagged rather than solved. 300-page benchmark re-run against 29 s / 470 MB |
| ✅ **3. Long-doc pipeline** | `--jobs` pool, ordered assembly, single-writer manifest, auto-resume, atomic writes, progress, page markers | Killing a run mid-way and re-running resumes. Output is identical for `--jobs 1` and `--jobs 8`: byte-for-byte for every engine at default settings, except the `generated:` timestamp line |
| ✅ **4. Server management** | `setup` (venv, weights, RAM check), start/stop/status/restart, readiness probe | Start, stop and status work for each engine. A killed server is detected and restarted |
| ✅ **5. Model engines** | olmOCR (v4 prompt), dots (layout JSON → Markdown), PaddleOCR-VL (full page + table crops with OTSL→GFM), Apple side-pass, timeout/restart, truncation, loop detection, fallback | Each engine passes the eval set |
| ✅ **6. Quality** | Eval harness, tuned defaults, opt-in legal cleanup | CER targets met on the eval set |

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
  - **olmOCR scores 0/25 on full-size transcripts** because it drops the line-number margin, so it printed a hint to use paddleocr-vl (superseded 2026-10-08: hints now point to `line_numbers.unverified` and recommend dots-ocr). dots scores 25/25 and 100/100.
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
  - **Benchmark target:** the Phase 3 300-page figure (28.4 s) will be re-measured under the limit, and the new figure becomes the target. (Done, see the 2026-10-08 note below: 84-96 s.)

- 2026-10-08, **Phase 6 done.** Eval harness, tuned defaults and opt-in legal cleanup are in; results are in [docs/eval.md](docs/eval.md) and the full day is summarised in [docs/overnight-report.md](docs/overnight-report.md).

- 2026-10-08, **Apple Vision limit of 2 kept, with the corrected cost (your decision, second time).**
  - **Quiet-machine 300-page benchmark** (`spike/out/bench300/big.pdf`, binary at de8b78a+, nothing else running): unlimited (`OCR_VISION_MAX_CONCURRENT=64`) 29.0 s; limit 3 57.3 s; limit 2 (default) 84-96 s, **about 3x slower than unlimited**. Worker peak memory at the limit of 2 is 192 MB (`OCR_SCAN_CHILD=1 /usr/bin/time -l`; under the supervisor `/usr/bin/time` sees only the parent, about 2 MB). All 300 pages correct: case number 200/200, two-column 100/100.
  - The earlier "about 1.36-1.5x" estimate was measured while dots was using the GPU and understated the cost. You kept the limit of 2 after seeing the corrected figure.
  - **Crash test** (`spike/repro_init_race.sh`, bench mode, 12 threads, dots running concurrently; `spike/out/cap4_crash_test.log`): unlimited control 6 crashes in 100 launches (6%; codes 139 x5 and 133); limit 4 7 in 300 (2.3%); limit 2 0 in 200 (100 bench + 100 earlier, bench and scan). Every real crash happened within 0.81 s of launch. The supervisor retries from checkpoints.

- 2026-10-08, **Transcript line-number re-anchoring** (merged de8b78a, hints updated 40562ab). Lines exact on held-out seeds 40-42 (`transcript4up`), before to after: dots 68% to 99.9%, olmOCR 30% to 99.9%, Apple 38% to 97.4%, paddleocr-vl 25% to 94.9%. Paddle misses the 95% target because of its own text errors (dropped, repeated or invented lines). Seeds 10-12 were burned by retuning, so only the 40-42 numbers count. Hints now point to `line_numbers.unverified` and recommend dots-ocr for transcripts. Evidence for the Paddle renumbering finding: `spike/transcript_paddle_probe.py`.

- 2026-10-08, **Per-engine default DPI** (commit 2631d47; docs/eval.md): apple 200, paddleocr-vl 150, dots-ocr 100; olmOCR is fixed at 1288 px. Apple stays at 200 by your decision although 150 scored better (tune recall 98.2% vs 97.4%; accept fax 96.3% vs 88.5%).

- 2026-10-08, **Public set** (20 CourtListener filings, 230 pages; `eval/out/public/compare.txt`, judged by Jev, `eval/JUDGE.md`):

  | Engine | Mean readability | Pages < 3.5 | Garbled |
  |---|---|---|---|
  | dots | 4.39 | 7% | 11% |
  | paddleocr-vl | 4.16 | 19% | 28% |
  | Apple | 3.57 | 48% | 73% |

  - dots took about 2 h for 230 pages (partly contended); Apple about 2 min. One dots document exited 1 (`scan-removed-3`, flagged pages). The first 5 dots documents were first run at 200 DPI with an older binary; all 20 were redone at the 100 DPI default (old partial outputs: `eval/out/public/dots-ocr-200dpi-partial`).
  - Paddle repeated text on 4% of real pages, below the loop detector's 6-repeat threshold.
  - **Apple failure modes on real filings** (eval/PUBLIC_SET.md): column split firing on single-column pages and cutting lines mid-word; full-page transcripts not detected; pilcrow never read correctly (0/48) and section sign read as S or $ on typewriter scans; Garamond r/t confusion; pleading prose emitted as a table and repeated; `flagged_pages` stayed empty even where the judge rated 67% of deposition pages poor.

- 2026-10-08, **Other Phase 6 findings** (docs/eval.md):
  - **Paddle pleading-table bug:** a full-width table crop pulls margin line numbers into the table as an extra first column.
  - **Apple Bates accuracy** on accept pleading pages is 88.6%; not investigated.
  - **Concurrency lesson:** two model servers sharing the GPU dropped combined throughput below half of one server; parallel stage-B runs were abandoned.
  - The Mac slept 00:31-07:45, pausing all work for about 7 h.
  - Full 500-page olmOCR and dots runs were skipped (60-page slices only), olmOCR was not run on the public filings, and handwriting is not scored.

Still open:

1. **Hardware floor** (vmmap physical footprint, Phase 6 re-measure): olmOCR 8-bit 11.2 GB, dots 6.8 GB (at 100 DPI), Paddle bf16 4.0 GB. Setup recommends `--quant 4bit` for olmOCR below 24 GB of RAM, and refuses 8-bit below 16 GB without `--force`.
2. **Text-layer trust.** Bad hidden OCR layers from older scanners exist. The `auto` heuristics need tuning on real samples.
3. **Throughput expectations (measured, quiet machine, Phase 6).** A 300-page scan takes about 1.5 minutes with Apple at the Vision limit of 2, about 27 minutes with Paddle, about 58 minutes with dots at 100 DPI and about 80 minutes with olmOCR. The CLI shows an ETA, and the README states these figures. (Earlier estimates of 1 min, 12 min and 2.9 h are superseded.)
4. **Apple accuracy on legal numbers.** On one page, Vision's errors depended on DPI. Phase 2 tunes this on the eval set. A dual-DPI cross-check that flags pages where numbers disagree is a candidate.
5. **Privacy.** Everything is local. Servers bind to localhost only. There is no telemetry. After setup, HF downloads are the only network access, and `HF_HUB_OFFLINE=1` is set at runtime.
6. **Follow-ups from Phase 6**, in priority order:
   1. Apple column split firing on single-column real filings.
   2. Paddle pleading-table margin column.
   3. Pilcrow and section-sign misreads (Apple).
   4. Paddle repetition below the loop-detector threshold on real pages.
   5. Apple Bates accuracy of 88.6% on pleadings.
   6. Apple `flagged_pages` never set on bad pages; consider wiring the judge's signals or a heuristic.
   7. Pleading margin-number inconsistency across engines.
   8. olmOCR rotated-page Bates gap.
   9. A text-layer path with layout handling.
   10. The flaky `tests/resume.rs::model_directory_output_does_not_depend_on_jobs`, which failed once at server start.
   11. Report the Vision crash to Apple via Feedback Assistant, with `spike/repro_init_race.sh`.
7. **Pending on other machines:** the Linux vLLM reference ([docs/linux-reference-eval.md](docs/linux-reference-eval.md), branch `linux-reference-eval`) and the Azure Document Intelligence comparison ([docs/azure-eval.md](docs/azure-eval.md), branch `azure-eval`).
