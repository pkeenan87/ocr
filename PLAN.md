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
  --dry-run                                    (print the input → output mapping and exit)
  --use-text-layer auto|never|always           (born-digital PDFs: use embedded text instead of OCR)
  --legal-cleanup                              (opt-in, off by default: strip pleading line numbers / running headers)
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
| **2. Apple engine end-to-end** | Swift bridge gains page count, per-page text layer, and render-page-*n*-then-Vision (white-filled `CGContext`, no temp PNGs, one document per worker, multi-page TIFF via ImageIO). Vision → Markdown, `--no-front-matter`, file writing. **Reading order:** keep Vision's paragraph order and insert tables/lists before the first paragraph below them; a global top-left sort interleaves columns | A scanned multi-page PDF and a folder of mixed images produce correct `.md` files. On the clean fixture **and** on a degraded-scan fixture (image-only, skew, noise, JPEG), the case number and every dollar amount are exact through the production render path. This re-checks the 200 DPI default. Two-column text comes out column by column |
| **3. Long-doc pipeline** | `--jobs` pool, ordered assembly, single-writer manifest, auto-resume, atomic writes, progress, page markers | Killing a run mid-way and re-running resumes. Output is identical for `--jobs 1` and `--jobs 8`: byte-for-byte for every engine at default settings |
| **4. Server management** | `setup` (venv, weights, RAM check), start/stop/status/restart, readiness probe | Start, stop and status work for each engine. A killed server is detected and restarted |
| **5. Model engines** | olmOCR (v4 prompt), dots (layout JSON → Markdown), PaddleOCR-VL (full page + table crops with OTSL→GFM), Apple side-pass, timeout/restart, truncation, loop detection, fallback | Each engine passes the eval set |
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

Still open:

1. **Hardware floor** (vmmap physical footprint): olmOCR 8-bit 10.9 GB, dots 7.3 GB, Paddle bf16 3.8 GB. Setup recommends `--quant 4bit` for olmOCR below 24 GB of RAM, and refuses 8-bit below 16 GB without `--force`.
2. **Text-layer trust.** Bad hidden OCR layers from older scanners exist. The `auto` heuristics need tuning on real samples.
3. **Throughput expectations.** A 300-page scan takes about 1 minute with Apple, 12 minutes with Paddle, 80 minutes with olmOCR and 2.9 hours with dots. The CLI shows an ETA, and the README should state these figures.
4. **Apple accuracy on legal numbers.** On one page, Vision's errors depended on DPI. Phase 2 tunes this on the eval set. A dual-DPI cross-check that flags pages where numbers disagree is a candidate.
5. **Privacy.** Everything is local. Servers bind to localhost only. There is no telemetry. After setup, HF downloads are the only network access, and `HF_HUB_OFFLINE=1` is set at runtime.
