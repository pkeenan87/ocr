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
| Apple | Built in | — | `RecognizeDocumentsRequest` with `[en-US]`, auto-detect off and **language correction off**, which caused a `cv`→`cr` error | **150 DPI default** (was about 200 until the 2026-10-08 "Resolved" note in §8): on the fixture, 300 and 400 DPI were *less* accurate. Tuned in Phase 2, about 0.15 s/page at 8 jobs. The bridge rebuilds reading order from the bboxes and drops paragraphs that duplicate table or list text. Margin line numbers are filtered by bbox. List markers come from `markerString` |

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

- 2026-10-08, **Resolved: Apple default switched to 150 DPI (your decision, later the same day).** Evidence: tune seeds 0-9 token recall 98.2% vs 97.4%, CER 3.10% vs 3.29%; accept seeds 10-39 token recall 98.4% vs 97.4% (fax 96.3% vs 88.5%); pleading CER 2.1% vs 1.7% and pleading Bates 87.6% vs 88.6% are slightly worse. The four Apple goldens were regenerated (only `Case No.3:26-cv-04417` lost a space; all critical tokens exact). 300-page benchmark at 150: 85-87 s, worker 178 MB. Supersedes the next note, kept for history: **Apple stays at 200 DPI (your decision).** On tune seeds 0–9, 150 DPI scored slightly better (token recall 98.2% vs 97.4%, CER 3.10% vs 3.29%). The default stays at 200 so that the golden outputs, and the behaviour they guard, don't change. `--dpi 150` remains available.

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

- 2026-10-08, **Per-engine default DPI** (commit 2631d47; docs/eval.md): apple 200 at the time (now 150, see the Resolved note above), paddleocr-vl 150, dots-ocr 100; olmOCR is fixed at 1288 px. Apple was kept at 200 by your decision although 150 scored better (tune recall 98.2% vs 97.4%; accept fax 96.3% vs 88.5%).

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
   4. Paddle repetition below the loop-detector threshold on real pages. On Linux (2026-10-18, the 20 public filings): only 1 of 230 pages has a short back-to-back repeat the detector misses (3 repeats of a table cell). The real misses were loops that ran into max_tokens; fixed, see the 2026-10-18 note.
   5. Apple Bates accuracy of 88.6% on pleadings.
   6. Apple `flagged_pages` never set on bad pages; consider wiring the judge's signals or a heuristic.
   7. Pleading margin-number inconsistency across engines.
   8. olmOCR rotated-page Bates gap. Off the Mac, fixed 2026-10-18: the corner requests are taken from the turned page. dots.ocr, which has no rotation signal, still misses the Bates number on a sideways page.
   9. A text-layer path with layout handling.
   10. The flaky `tests/resume.rs::model_directory_output_does_not_depend_on_jobs`, which failed once at server start.
   11. Report the Vision crash to Apple via Feedback Assistant, with `spike/repro_init_race.sh`.
   12. Off the Mac, a body token in a page's last lines (`Invoice No. INV-24337`) is taken as the Bates number when the model dropped the footer, and the corner request is then skipped (`corner_bates`). Seen with olmOCR and dots.ocr on seed10 pleading. Fixed for tokens followed by a lowercase word (after any `,;:)`); pleading Bates is now 11/11 for both engines. A token that ends a sentence (`INV-24337.`) is still taken.
7. **Pending on other machines:** the Azure Document Intelligence comparison ([docs/azure-eval.md](docs/azure-eval.md), branch `azure-eval`). The Linux vLLM reference ran on 2026-10-14 ([docs/linux-reference-eval-results.md](docs/linux-reference-eval-results.md)).

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
| ✅ **7a. Backend spike** (no Rust; run on the Ryzen CPU build and, for CUDA, the RTX 5060 Ti instead of the 3090, recorded in docs/spike.md) | Pinned llama-server build with each GGUF. Run the four prompts (`OCR:`, `Table Recognition:`, olmOCR v4 YAML, `prompt_layout_all_en`) through llama-server's chat templates. Test `-np 2` with images. Measure VRAM and s/page per engine and quant. Compare output with the Mac's MLX output and the vLLM reference. Send olmOCR a footer-strip crop: does the Bates number come back? (It is trained to drop footers, and option (b) in §9.5 depends on this.) Settle the Vulkan device probe | `docs/spike.md` records: prompt compatibility (same output shape as MLX), whether parallel image slots work, VRAM floors, speeds. Any engine that fails is documented with its blocker |
| ✅ **7b. Build portability** | `build.rs` and the FFI half of `vision.rs` gated on `target_os = "macos"` (types stay shared). `.cargo/config.toml` rustflags moved under `[target.aarch64-apple-darwin]`. Supervisor macOS-only. `flock` → `File::lock`. Process and memory calls → `sysinfo` / job object. `Engine::Apple` hidden off the Mac | `cargo build` and the unit tests pass on Linux (Mac-only tests gated). `cargo check --target x86_64-pc-windows-msvc` passes. Mac tests unchanged |
| ✅ **7c. Portable document layer** (Mac comparison pending) | `Document` trait over `document_info`, `text_layer`, `render_gray`, `render_png_fit`. pdfium + `image`/`tiff` implementation, including multi-page TIFF and rotation | On the non-table fixtures, `regions.rs` makes the same column and band decisions as on the Mac (renders need not be byte-identical). Table pages differ by design: there are no Vision table masks |
| ✅ **7d-1. llama-server backend** | `Backend` (mlx-vlm / llama-server) in config, `server.json` and checkpoints. `server.json` gets an explicit `slots` field instead of reading `--max-num-seqs` from argv. Checkpoints record a generic runtime version instead of `mlx_vlm`, under the same key so Mac checkpoints stay valid. Setup downloads with `curl` and verifies the sha256 of: the pinned llama.cpp archive (plus cudart for CUDA), pdfium, and the GGUF weights pinned by HF revision. Minimal GPU probe to pick the archive, with an `OCR_LLAMA_VARIANT` override. `-c` per engine from `Limits` x slots | Start, stop, status and restart pass with the fake server on Linux. A real paddleocr-vl server starts on this machine (Vulkan or CPU) |
| ✅ **7d-2. Engine layer off the Mac** (GPU acceptance passed 2026-10-14, RTX 5060 Ti) | A non-Mac `side_pass` (bitmap regions, no blocks). `find_bates` runs on the model's output. Paddle's table-crop branch is off. Failed pages are flagged with no fallback text. The olmOCR footer-strip request (§9.5) | `paddleocr-vl`, `olmocr` and `dots-ocr` each scan `three_pages.pdf` on the Linux GPU machine, with Bates numbers in the page markers |
| ✅ **7e. GPU gate** (CUDA path checked 2026-10-14 on an RTX 5060 Ti) | Hardware probe, build selection, VRAM tiers, the no-GPU warning | This AMD-iGPU machine gets the warning and still runs on CPU. The 3090 machine is silent and picks CUDA |
| **7f. Windows** (CI green on Linux and Windows; a real Windows + NVIDIA run is pending, no machine yet) | `.exe` paths, job-object kill, CI build and unit tests on `windows-latest` (model tests use `tests/fake_server.py`) | A scan runs end-to-end on a Windows + NVIDIA machine |
| ✅ **7g. Eval and docs** (2026-10-18, margin: no more than 1 point of CER worse than the Mac, except Paddle tables, documented with dots/olmOCR recommended) | Run the eval set (seeds 10–12) on Linux; compare with the Mac and vLLM numbers. README: Linux/Windows requirements, speeds, VRAM table | CER within an agreed margin of the Mac numbers per engine |

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

- 2026-10-08, **7d-2, PaddleOCR-VL part built** (the olmOCR footer strip is pending its test). Decisions made in the implementation:
  - **Side-pass off the Mac:** `pipeline::side_pass` returns bitmap regions with no Vision blocks. `choose_plan` therefore still splits multi-column pages into column and band crops, and with no blocks, the table-crop branch switches itself off. No transcript detection.
  - **Bates numbers:** taken from the last 4 non-empty lines of the model's text. The bottom-most, right-most match of the existing Bates regex wins. Off the Mac only; the Mac keeps Vision's.
  - **Failed pages off the Mac** have no Apple text to fall back to, so they are recorded as failed and a re-run retries them.
  - **Checked on this machine** (CPU build, PaddleOCR-VL 1.6):
    - The page markers of `three_pages.pdf` and `two_column.pdf` match the Mac goldens exactly, Bates included.
    - `two_column` and `narrow_unequal_seed2` are split by column: narrow_unequal has 42/42 lines in order. On the Mac, Paddle full-page collapsed to 10–28 lines on this layout.
    - `legal.pdf` (with its table) is not split.
    - About 4–8 s a page.

- 2026-10-08, **7d-2 olmOCR part built.** Option (b) of §9.5, the footer strip, failed: olmOCR returns nothing for it. **Your decision:** corner crops instead. Off the Mac, when the page text has no Bates number, olmOCR is asked for the bottom-right corner, then the bottom-left (256 tokens each), and the first match wins. A failed or empty corner is skipped; only a stopped server ends the page.
  - **Results:** correct on the clean PDF, the scan, a bottom-left stamp and 4 generated production pages. Correctly none on a page without one.
  - **Known limit:** a Bates number centred under the footer is missed.
  - Details are in docs/spike.md, "olmOCR footer strip and Bates corners".
  - **Not run yet:** dots-ocr. It keeps Page-footer text, so the text search should find Bates numbers, but this is unverified.

- 2026-10-08, **7f, first part.** This machine can't run Windows code: no Wine and no MinGW. The Windows tests therefore run in GitHub Actions (`.github/workflows/ci.yml`). It runs `fmt`, `clippy -D warnings` and `cargo test` on `ubuntu-latest` and `windows-latest`, each with pinned, checksum-verified pdfium. macOS stays manual: it needs macOS 26 for Vision.
  - **Code fixes from a Windows audit:**
    - Atomic renames retry for about 250 ms on `PermissionDenied`, for when antivirus or the search indexer holds the target (`checkpoint::replace_file`, used by every atomic write and by downloads).
    - `curl`/`tar` are taken from System32 first, because a GNU `tar` from Git or MSYS on `PATH` can't unpack zips.
    - Stop is `taskkill /T /F`, which kills the process tree, falling back to TerminateProcess.
    - `is_ours` is case-insensitive and falls back to the process name when Windows hides the command line.
    - The console is switched to UTF-8 (`SetConsoleOutputCP(65001)`), so `⚠` and `—` print correctly.
  - **Test changes:**
    - `tests/fake_server.cmd` wraps the fake server on Windows; `.gitattributes` keeps its CRLF line endings.
    - `fake_alive` checks the port instead of parsing `ps` output.
    - The foreign-pid test uses `ping -n 60` on Windows.
    - `tests/cli.rs` compares paths with `\` normalised to `/`.
  - **Windows coverage** comes from `tests/cli.rs`, `tests/server.rs`, `tests/document.rs` and the unit tests. The engine tests that use the fake server need Vision and stay macOS-only.
  - **Done when** (unchanged): the CI is green, and a scan runs on a Windows machine with an NVIDIA GPU.

- 2026-10-08, **7g, first Linux eval** (PaddleOCR-VL 1.6, CPU build, accept seeds 10–12, no production family; `eval/out/results/paddle-linux-cpu*.json`):

  | Family | Pages | Column crops first | Full page first (now) |
  |---|---|---|---|
  | fax | 6 | 0.0% | 0.0% |
  | contract | 14 | 0.1% | 0.0% |
  | pleading | 11 | 5.5% | 5.5% (Bates 100%) |
  | table | 6 | 52.0% | 12.3% (recall 100%) |
  | transcript4up | 9 | 43.8% | 56.6% (lines 28.8%) |

  - **Decision made for you:** off the Mac, PaddleOCR-VL reads the full page first and keeps the column crops for a loop or truncation, as olmOCR does. Without Vision's table masks, the column split cut tables into fragments.
  - **Your decision:** 4-up transcripts stay on full page until the Mac's PaddleOCR-VL transcript4up numbers on the same seeds are in. If the Mac is about as poor, this is a shared limit; if not, detect the 2x2 layout and crop the panels.
  - `eval/run.py` runs off the Mac now: no BSD `time` or `vmmap`; it reads `/proc` VmHWM, `nvidia-smi` and `ru_maxrss`.

- 2026-10-08, **7f: CI green on Windows.** All 142 tests pass on `windows-latest` and `ubuntu-latest` (run 37832290941); the one ignored test needs a real model server. The first runs found:
  - **A real bug.** A detached server inherited `ocr`'s stdout/stderr handles, so anything reading `ocr`'s output (the tests, a pipe to `tee`) waited for the server to exit. Fixed: `ocr` clears the inherit flag on its std handles before spawning the server.
  - **Git's CRLF conversion** broke the verbatim-prompt tests. Fixed: `spike/upstream/** -text`.
  - **One path test** compared paths with `/`.
  - **Git Bash's GNU `tar`** in the CI step (not in `ocr`).
  
  **Still open:** a scan on a real Windows machine with an NVIDIA GPU. You have none yet.

- 2026-10-08, **after merging master.** The Mac PaddleOCR-VL accept numbers (docs/eval.md, seeds 10–19) settle two questions:
  - **4-up transcripts:** 51.1% CER on the Mac, against 56.6% here (seeds 10–12). A shared limit, not a regression of the port, so it is closed as documented; use dots for transcripts. Master's line-number re-anchoring needs the Vision line geometry: off the Mac no page is detected as a transcript, so it does not run.
  - **Tables:** 0.0% on the Mac, against 12.3% here. A real regression off the Mac: there are no Vision table boxes to crop for `Table Recognition:`. **Follow-up:** find table regions from the bitmap (rule lines are already detected in `regions.rs`), or use dots' layout boxes.

- 2026-10-14, **GPU run (RTX 5060 Ti 16 GB, Blackwell sm_120, driver 610.57, CUDA 13.3).** This machine now has the 5060 Ti; it was used for the GPU acceptance runs instead of the 3090. Results in `eval/out/results/*-linux-cuda.json`.
  - **7e bug fixed:** driver 610's `nvidia-smi` header says `CUDA UMD Version: 13.3`, not `CUDA Version:`, so the probe found no CUDA and setup picked Vulkan. `parse_cuda_major` accepts both. Setup now picks cuda13 silently; b11510's cuda-13.4 cudart runs on the 13.3 driver, and its kernels run on sm_120.
  - **7d-2 bug fixed:** dots' first request is `prompt_ocr`, which upstream defines as leaving out Page-header and Page-footer, so off the Mac dots had no Bates numbers (`three_pages.pdf` gave none). dots now gets the olmOCR corner requests, moved to `Ctx::corner_bates`: `prompt_ocr`, then the layout prompt, on each corner (docs/spike.md, "dots Bates corners"). `three_pages.pdf` now matches the golden markers for all three engines.
  - **Eval bug fixed:** `eval/score.py` reused `m` (the counters) for the Bates regex match, so any fixture with a Bates page before its last page crashed the scorer (introduced in ab7bdfb, 2026-10-11, after the Mac runs).
  - **Accept seeds 10–12 on CUDA** (CER / token recall; Mac numbers from docs/eval.md in brackets):

    | Family | paddleocr-vl | olmocr Q8_0 | dots-ocr (dots.ocr Q8_0) |
    |---|---|---|---|
    | pleading | 0.9% / 98.4%, Bates 100% [13.3%] | 0.0% / 98.9%, Bates 81.8% [0.0%, 90.9%] | 0.1% / 99.5%, Bates 90.9% [0.0% dots.mocr] |
    | contract | 0.0% / 100% [0.1%] | 0.0% / 100% [0.0%] | 0.0% / 100% [0.2%] |
    | transcript4up | 39.1% / 91.5%, lines 34.3% [51.1%] | 11.6% / 96.9%, lines 25.6% [11.2%] | 2.0% / 100%, lines 76.3% [1.2%, 68.1%] |
    | fax | 0.0% / 100% [0.0%] | 0.0% / 100% [0.0%] | 1.2% / 100% [6.3%] |
    | table | 12.3% / 100% [0.0%] | 0.0% / 100% [0.1%] | 0.0% / 100% [0.1%] |
    | GPU memory | 2.4 GB | 10.2 GB | 4.4 GB |
    | pages/s | 0.16–0.71 | 0.03–0.11 | 0.13–0.43 |

    The Mac's Paddle pleading figure is seeds 10–19; the rest are seeds 10–12. The Paddle table gap is the known missing table crops (no Vision table boxes); vLLM at full precision gives the same 12.3%, so it is not the GGUF.
  - **7g:** CER is no more than 1 point worse than the Mac's everywhere except Paddle tables (12.3% against 0.0%, the missing Vision table crops). Three cells are more than 1 point better than the Mac: Paddle pleading (0.9% against 13.3%, different seed ranges), Paddle transcripts (39.1% against 51.1%) and dots fax (1.2% against 6.3%, dots.ocr against dots.mocr). olmOCR pleading Bates is 81.8% against the Mac's 90.9% (9/11 against 10/11); both misses are wrong, not missing. A body invoice number (`INV-24337`) in the last lines of a page whose footer olmOCR dropped is taken as the Bates number, and the corner request is then skipped (docs/linux-reference-eval-results.md). Not marked done: the margin is yours to agree.
  - **vLLM reference** (docs/linux-reference-eval-results.md): no measurable quantization loss for dots (8-bit dots.mocr against bf16). olmOCR ran as FP8 because bf16 does not fit 16 GB, so its quantization question stays open until a 24 GB card. PaddleOCR-VL is unquantized everywhere.

- 2026-10-14, **indra-ocr engine.** A fifth engine, `datalab-to/indra-ocr` 0.3.1: a CPU ONNX pipeline, not a VLM. Decisions made in the implementation:
  - **No server.** It runs as a helper process per document (`src/indra_helper.py`, embedded and written to `<state>/indra/helper.py` by setup, run with the venv at `<state>/indra/venv`). It is not a `ScanEngine` and `Ctx::post` is not involved; `main.rs` has a third scan branch beside Apple and the model engines. `model_gate` is not used: the gate is `[engines.indra-ocr] installed = true`, else exit 2 with `ocr server setup --model indra-ocr`. `server start --model indra-ocr` prints that no server is needed and exits 0. `default_server_model` ignores it (`Engine::has_server`).
  - **Shared pipeline, one new hook.** `run::Backend` gained `open(path, kind, pending, settings)`, called once per file after the checkpoints are loaded, with the pages still to do. Indra's `open` records the plan; its `prepare` starts the helper on the first page it is asked for (`--pages` = the pending pages) and then reads one JSON line per page. `producers` is 1 and there are no consumers, so only one helper runs at a time even with four files open. A page the helper reports as an error, or never delivers because it died, is a failed page: not checkpointed, retried by a re-run. `Err` from `open` fails the file.
  - **Pages, not `convert()`.** The package iterates a whole document, so the helper calls `Converter.pdf_page(path, doc, index)` (wrapped in `Converter._safe`) for just the wanted pages; images use `read_images` and `OrtOCR.__call__`. This is internal API of `indra-ocr` 0.3.1, which is why the version is pinned (`engines::INDRA_PIN`).
  - **Weights.** The default pipeline loads seven files, not the four `models.FILES` requires: `OrtOCR` also opens `order`, `orient` and `script`+`rec_cjk` when present. Pinned at HF revision `5993a95…` with sha256 in `engines::INDRA_FILES` (meta.json's was computed from the download; the others are the LFS oids), fetched by the existing `fetch::download` (curl, resume, hash) into `<state>/models/datalab-to/indra-ocr/`. `fetch` is no longer macOS-excluded. The helper sets `HF_HUB_OFFLINE=1` and passes `path=`; the package never downloads. Math is off.
  - **Setup.** Skips the llama.cpp probe and download, the GPU/VRAM check and the CPU refusal, and leaves `[runtime]` alone. **It still installs pdfium off the Mac**, because `ocr` itself counts and reads pages with pdfium (`vision::document_info`) before the helper runs. `find_uv` and `UV_INSTALL` work on Linux and Windows (`uv.exe`, the PowerShell installer, `Scripts\python.exe`); the mlx-vlm venv code shares `ensure_venv_pkg`. `--quant` with `--model indra-ocr` is a usage error (ignored under `--all`); `--model-version` is the usual "no selectable version". Footprint is a nominal 0.5 GB so the RAM check passes trivially (it is not run for indra).
  - **Body and Bates.** Page-Header and Page-Footer blocks are removed from the helper's markdown by dropping the paragraph equal to a furniture block's text (whitespace-normalised, one paragraph per block); a block merged into a longer paragraph would stay. HTML tables go through `htmltable::convert_tables`, then `clean_ocr`. Bates: the right-most match of the bottom-most Page-Footer block that has one, then Page-Header blocks, then `find_bates_in_text(body)`. `markdown::last_bates_in` is the new public wrapper over `bates_in`.
  - **Text layer.** `--use-text-layer always` is honoured as for the model engines (`pipeline::text_layer_page` in `open`; those pages never go to the helper). `auto` is the Apple-only corrected-Vision path and does not apply. The helper runs `mode="ocr"`, so the PDF's own text is never read by indra.
  - **Defaults.** `--dpi` 200 (`INDRA_DPI`); `--jobs` is the helper's ONNX thread count and defaults to all logical cores (`available_parallelism`). Checkpoint identity: `indra-ocr (datalab-to/indra-ocr@5993a95, indra-ocr 0.3.1)`.
  - **Tests.** `tests/indra.rs` with `tests/fake_indra.py` (`OCR_INDRA_CMD` replaces `<python> helper.py`, like `OCR_SERVER_CMD`); runs on Linux and Windows, needs `OCR_PDFIUM_LIB` for page counts. `eval/run.py` accepts `--engine indra-ocr` and skips the server peak and VRAM readings for it.
  - **Real run (Linux, 32 logical CPUs):** `spike/fixtures/three_pages.pdf` gives the golden's three markers (`page 1 · Bates SMITH000123`, `page 2`, `page 3 · Bates SMITH000123`) in 1.2 s. Not yet run: the Mac and Windows paths beyond `scripts/xcheck.sh`.
  - **Eval (accept seeds 10–12, `eval/out/results/indra-linux-cpu.json`):**

    | Family | Pages | CER | Token recall | Bates | Transcript lines | Pages/s |
    |---|---|---|---|---|---|---|
    | pleading | 11 | 9.9% | 100% | 100% | - | 3.4 |
    | contract | 14 | 0.0% | 100% | - | - | 3.5 |
    | transcript4up | 9 | 6.8% | 100% | - | 10.6% | 2.0 |
    | fax | 6 | 0.1% | 100% | - | - | 3.2 |
    | table | 6 | 0.0% | 100% | - | - | 2.5 |

    About 10x the GPU engines' speed on the CPU alone. The pleading CER is the margin line numbers, which indra merges into the text blocks. Transcript line numbers are mostly lost.

- 2026-10-18, **Linux 1.0.** Release work on branch `linux-1.0`. Decisions (yours): merge #45 first; document Paddle's table gap and recommend dots-ocr or olmOCR for tables rather than build a bitmap table detector; Linux is supported, Windows is CI-only and not yet supported, indra-ocr ships with its license note; a prebuilt Linux tarball with sha256 from a release workflow.
  - **Fixes:**
    - Bates corners gained the bottom centre (a third request); a centred stamp was missed. On `legal.pdf` restamped at bottom centre (4 pages), olmOCR, dots and Paddle read 4/4. `three_pages.pdf` page 2 (no stamp) still gets none.
    - The loop retry sent `repetition_penalty`, which llama-server ignores (it reads `repeat_penalty`; the b11510 libraries contain no other name), so off the Mac a looping page was retried unchanged. Checked on a live server: `repetition_penalty: 1.15` gave output identical to no penalty, `repeat_penalty: 1.15` changed it.
    - olmOCR's Bates corners are taken from the turned page when the model says the rotation is invalid (follow-up 8): `tests/fixtures/legal_rot90.pdf` had no Bates, now `SMITH000123`.
    - Loops cut off at max_tokens (Paddle and olmOCR, both platforms). Paddle on the 20 public filings (230 pages, Linux) lost 3 pages as `output truncated at max_tokens`; all three were loops. A reply that looped and hit max_tokens was retried with more tokens and no penalty, and two loops escaped the detector: `By _____` (two distinct letters, under the rule's four) and a condensed transcript counting `[801] MR. X: It's 36 pages.`, `[802] …`. A cut-off reply is now checked with `has_cut_off_loop` (one letter or digit, numbers may differ) and a loop gets the penalty. ctr-nissan-dealer p1 and depo-christoforo-lupo p5 now succeed; lupo p1 (a 4-up condensed transcript) still loops under the penalty and is flagged as failed, as off the Mac there is no Vision fallback. Rerun over all 230 pages (`eval/out/public/paddleocr-vl-linux-b`): failed pages 3 → 1, and the 18 other documents byte-identical apart from `generated:`.
    - CI runs the tests in release mode: the un-gated model-engine tests took 25 minutes in a debug build on the runners (30-minute limit), and `tests/paddle.rs` used `fake_server.py` directly, which Windows cannot start; it now shares `common::FAKE` (the `.cmd` wrapper there).
  - **Eval** (seeds 10–12, RTX 5060 Ti, CUDA 13; `eval/out/results/*-linux-cuda-1.0.json`; Paddle and olmOCR at `312f100`, rerun at `e551528` after the cut-off-loop fix with identical results (`*-linux-cuda-1.0b.json`); dots at `0b8d44b`, which differs from the release only in code dots does not run). CER / token recall:

    | Family | paddleocr-vl | olmocr | dots-ocr (dots.ocr) |
    |---|---|---|---|
    | pleading | 0.9% / 98.4%, Bates 100% | 0.0% / 100%, Bates 100% | 0.1% / 100%, Bates 100% |
    | contract | 0.0% / 100% | 0.0% / 100% | 0.0% / 100% |
    | transcript4up | 39.1% / 91.5%, lines 34.3% | 11.6% / 96.9%, lines 25.6% | 2.0% / 100%, lines 76.3% |
    | fax | 0.0% / 100% | 0.0% / 100% | 1.2% / 100% |
    | table | 12.3% / 100% | 0.0% / 100% | 0.0% / 100% |
    | GPU memory | 2.4 GB | 10.2 GB | 4.4 GB |
    | pages/s | 0.16–0.70 | 0.03–0.09 | 0.12–0.37 |

    Same CER as the 2026-10-14 run everywhere; pleading Bates rose from 81.8% (olmOCR) and 90.9% (dots) to 100%. In the 15 runs no page failed or was flagged and no `ocr` process crashed, so the Apple-Silicon-derived timeouts did not cost a page on this GPU (a timeout that succeeded on its retry would not show: the server log is rewritten at each start). GPU memory matches the README estimates.

