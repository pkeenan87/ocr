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
    --no-symbol-fix            Keep Vision's readings of `¶` and `§` in citations (`Compl. 9 60`)
    --no-front-matter          Omit the YAML header
    --verify-json              Also write <stem>.verify.json: values to double-check, and pages for a second reader
    --verify-fields <MODE>     flagged (default) or all: which unconfirmed values need a second reading
    --use-text-layer <MODE>    auto, never, always (default: auto for apple, never for model engines)
    --legal-cleanup            Strip running headers and footers repeated across pages (opt-in)
    --dpi <N>                  Render resolution, 72-600 (default 150; dots-ocr 100)
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
    ocr scan --input-file depo.pdf --model dots-ocr              # deposition transcript
    ocr scan --input-file big.pdf --pages 1-50,75
    ocr scan --input-file big.pdf --jobs 4                       # Apple: 4 pages at once
    ocr scan --input-file big.pdf --overwrite                    # discard output and checkpoints
    ocr scan --input-file big.pdf --legal-cleanup                # strip repeated headers/footers
    ocr scan --input-file big.pdf --dpi 150
    ocr scan --input-file big.pdf --use-text-layer always        # raw embedded text, no OCR (fastest, no layout)
    ocr scan --input-file big.pdf --no-page-markers --no-front-matter --no-currency-fix

Notes on the options:

- **`--model-version`** pins an older model release; it applies to `paddleocr-vl` and `dots-ocr` only.
- **`--jobs` / `--parallel`** is the total number of pages in flight. The default is the number of performance cores for Apple and 1 for model engines. A model server handles one request at a time (`--max-num-seqs 1`), so `--jobs` above 1 on a model engine only overlaps rendering, the Apple side-pass and assembly, and prints a note. Output is the same for `--jobs 1` and `--jobs 8`.
- **`--use-text-layer`**: with the apple engine the default is `auto`. Vision reads the page as usual (layout, tables, margin numbers, Bates), and then every block's words are aligned with the PDF's embedded text inside the block's box, so a born-digital page keeps Vision's structure with the PDF's exact characters (`Case No.3:26-cv-04417` becomes `Case No. 3:26-cv-04417`, `$3,000` read for `$8,000` becomes `$8,000`). The changes are counted per page in `embedded_text_fixes`. The safeguards:
  - A page is corrected only when at least 90% of its words agree with the embedded text, so a hidden layer that does not match the printed page (another draft, a bad scanner OCR layer) changes nothing.
  - A differing word is replaced only when it looks like a misread of the embedded word: at most a third of its characters differ. `thirty` never becomes `sixty`.
  - **Text in the layer that Vision did not see on the page is never inserted.** This covers words under a drawn redaction box, white or invisible text, and neighbouring text caught by a box.
  - **Text drawn invisibly (render mode 3, or fully transparent) is never a correction.** That is how OCR tools lay a text layer over a scan. Such text is another OCR's reading, not the document's characters (a 1985 note's layer reads `qction`).
  - **Column-split pages and transcripts are corrected too**, with stricter rules:
    - each box takes exactly the characters inside it, rather than PDFKit's drag-like selection, which takes in the neighbouring column's lines;
    - words of one or two characters change only between look-alike characters, so a misaligned line number `7` never becomes `6`.

    Transcript lines are corrected before their numbers are re-anchored to the page geometry.
  - **Letters and digits are never added.** Only punctuation that the PDF has and Vision missed can come back (`units essentially,` → `units—essentially,`).

  `never` is pure OCR (the default for model engines). `always` takes the raw embedded text instead of OCR and has no layout handling: margin line numbers come out inline, tables are flattened and the Bates number is lost.
- **`--dpi`** has a per-engine default tuned in Phase 6: Apple 150 (switched from 200 on 2026-10-08 by user decision: 150 scored better on the synthetic fixtures, mostly on fax scans; pleading CER and Bates were slightly worse, so use `--dpi 200` on clean pleadings if that matters), paddleocr-vl 150, dots-ocr 100 (as accurate as 200 and about 3x faster). olmOCR ignores `--dpi` (images are always 1288 px on the longest side) and says so.
- **`--legal-cleanup`** is opt-in because it changes citable text. A line is stripped when it sits in the top or bottom three lines of at least half the pages (minimum 3 pages); lines of 20 or more characters match fuzzily. Transcript pages, table rows, Bates numbers and page markers are never stripped. It runs at assembly time, so toggling it never redoes OCR.
- **`--no-currency-fix`**: Vision sometimes reads `$` as `S`. By default `S22,375.50` becomes `$22,375.50` on Apple pages (counted per page in the front matter). Model-engine output is not touched.
- **`--no-symbol-fix`**: Vision never outputs `¶` and sometimes reads `§` as `S` or `$`. By default, on Apple Vision text (and, with a model engine, only on pages that fell back to Apple), `Compl. 9 60` and `Id. T18` become `Compl. ¶ 60` and `Id. ¶ 18` (`99` becomes `¶¶`), and `U.S.C. $1441(b)` and `U.S.C. S 1367` become `U.S.C. § 1441(b)` and `U.S.C. § 1367`. Only these shapes are rewritten: the paragraph sign after a citation form such as `Id.`, `Compl.` or `Decl.` and before a number on the same line (never after `Ex.`), and the section sign after `U.S.C.`/`C.F.R.` (never before an amount such as `$75,000` or `$50.00`), `S` before a section with a subsection (`S1983(a)`), or `$` before one with two (`$1983(a)(1)`). A bare `$1441(b)` is left alone because it looks exactly like an amount with a list marker (`$150(a)`). Text-layer pages are not touched. The fix runs at assembly, so toggling it never redoes OCR; replacements are counted in `symbol_fixes`, and `--verbose` prints each one (`before -> after`).
- **`--file-type`**: pdf is `.pdf`; image is `.png .jpg .jpeg .tif .tiff .heic .bmp .gif .webp` (multi-page TIFF is a multi-page document). In directory mode it filters which files are picked up.

### Output naming

- Single file: `./<stem>.md` in the current directory unless `--output-file` is given. A name not ending in `.md` gets `.md` appended, with a note.
- Directory mode: `<output-dir>/<stem>.md`, with `--output-dir` defaulting to `<input-dir>/ocr-output/`. That folder is always skipped when walking the input. With `--recursive` the subfolder tree is mirrored.
- Collisions (`a.pdf` and `a.png` in one folder) become `a.pdf.md` and `a.png.md`.

### Resume

Each finished page is checkpointed to `<out>.md.ocr-partial/` next to the output file. If a run is interrupted (Ctrl-C, kill, crash, server loss), run the same command again and it resumes where it stopped; finished files in a directory run are skipped. A changed input, engine or setting (or `--overwrite`) starts fresh. The final `.md` is assembled from the checkpoints and written atomically. Page markers, front matter, `--pages`, `--legal-cleanup`, the `¶`/`§` repair and the suspect-page checks are applied at assembly, so changing them does not invalidate checkpoints. A file that ends with failed pages keeps its partial directory; re-running retries only those pages.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Partial: some pages failed, were flagged by the engine, or look wrong (`flagged_pages`) |
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
    symbol_fixes: {14: 3}
    suspect_pages: {141: [garbled]}
    flagged_pages: [88, 141]
    ---

    <!-- page 1 · Bates SMITH000123 -->
    # IN THE UNITED STATES DISTRICT COURT ...

Front matter fields (`--no-front-matter` omits the header):

- `source`: input file name. `pages`: page count.
- `engine`: engine, with the model repo for model engines.
- `generated`: UTC timestamp. The only line that differs between otherwise identical runs.
- `text_layer_pages`: pages taken from the raw embedded PDF text instead of OCR (`--use-text-layer always`).
- `embedded_text_fixes`: `{page: words}` for Apple pages corrected against the embedded text (`--use-text-layer auto`, the default); `0` means the page was checked and agreed.
- `column_split_pages`: pages where multi-column regions were detected and recognized column by column.
- `transcript_pages`: pages that look like deposition transcripts (numbered Q./A. lines).
- `line_numbers: {anchored: [...], unverified: [...]}` (only when there are transcript pages): pages whose line numbers were re-anchored to the page geometry, and pages where that failed and the engine's own numbering is kept. Check the unverified ones against the PDF.
- `table_pages` and `fallback_pages` (model engines): pages with a table recognized separately, and pages where the model failed and Apple Vision text was used instead.
- `currency_fixes` (Apple engine, instead of the two above): `{page: count}` of `S` to `$` corrections.
- `symbol_fixes`: `{page: count}` of `¶`/`§` restorations (see `--no-symbol-fix`).
- `suspect_pages`: `{page: [reasons]}` for pages whose text looks wrong although OCR succeeded (see below).
- `legal_cleanup` (with `--legal-cleanup`): number of stripped lines.
- `flagged_pages`: pages to check by hand: failed pages, fallbacks that lost quality, and every suspect page.

`suspect_pages` comes from checks on each finished page, for every engine. They were added because `flagged_pages` was empty on all 20 real court filings of [eval/PUBLIC_SET.md](eval/PUBLIC_SET.md) while about half their pages had visible damage. The reasons:

| Reason | Meaning |
|---|---|
| `garbled` | A run of mostly non-words, such as a line Vision turned into gibberish |
| `misreads` | Many scattered non-words on the page (at least 6, and 4% of checked words), such as a font Vision reads badly |
| `junk_lines` | Three or more lines of mostly symbols, such as a rotated sidebar or speckle |
| `duplicated` | A passage of 60 or more words appears twice on the page |
| `pleading_table` | A table whose first column is pleading line numbers: prose read as a table, or margin numbers pulled into a real table |
| `transcript_table` | Transcript lines (line numbers, Q. and A.) that came out inside a table; a real table on a transcript page, such as an exhibit list, does not count |
| `page_order` | A multi-up transcript's `Page N` headers are out of order |
| `mixed_case` | Three or more words with case flipping inside them (`MAeLoN`, `WeST`, `SMiTH`), typical of handwriting or fax noise in capitals; brand names such as `DocuSign` or `McDonald` do not count |

Words are checked against an English word list with legal terms added ([data/words-en.txt](data/words-en.txt), built by `scripts/make_wordlist.py`). Only lowercase words of four or more letters are checked, so names, citations, numbers and identifiers never count. Pages mostly in Spanish, French or Portuguese skip the word checks. Transcript pages whose line numbers could not be re-anchored are not suspect: they are listed in `line_numbers.unverified` with their own hint. On simulated misreads, a page is flagged as `misreads` reliably when about 8% of its words are misread, sometimes at 4%, and not at 1-2%; clean synthetic legal text has at most 1% non-words. The checks cannot see a wrong but plausible word or number (`8,000` read as `3,000`), a dropped sentence, or text in the wrong order that is still readable. An empty `suspect_pages` is not proof that a page is right.

Every page starts with a marker, `<!-- page 42 -->`, or with the Bates number when one is found at the bottom of the page: `<!-- page 3 · Bates ABC0000123 -->`. A page that failed outright appears as `<!-- page N: OCR failed: ... -->`. Page numbers are 1-based and always match the source document, also with `--pages`. When a model fails on a page (timeout, runaway repetition, empty or truncated output) it is retried and then falls back to Apple Vision text; that page is listed in `fallback_pages` and `flagged_pages`.

## Values to double-check (`--verify-json`)

`--verify-json` writes `<stem>.verify.json` next to the Markdown. It lists the values a reader must not get wrong, each with a status, and the pages a second reader should look at. It is built so that a tool can send only those pages to a paid OCR service such as Azure Document Intelligence (`azure_pages` uses its `pages` syntax), and so that an LLM can quote only confirmed values. The Markdown and the exit code are unchanged.

    ocr scan --input-file contract.pdf --verify-json                       # flagged fields only
    ocr scan --input-file contract.pdf --verify-json --verify-fields all   # every unconfirmed field

An example (abridged):

```json
{
  "source": "contract.pdf",
  "markdown": "contract.md",
  "engine": "apple (Vision RecognizeDocuments)",
  "verify_fields": "flagged",
  "azure_pages": "3,7-9",
  "summary": {"fields": 212, "verified": 180, "check": 9, "unchecked": 23,
              "pages_to_check": 4, "embedded_text_pages": 2},
  "pages": [
    {"page": 7, "bates": "ABC000107", "page_check": ["misreads"], "embedded_text": false,
     "second_reading": "azure",
     "fields": [
       {"kind": "amount", "text": "$48,250.00", "value": 48250.0, "line": 12,
        "status": "check", "why": ["suspect_page"]}
     ]}
  ]
}
```

- **Fields:** `amount`, `date` (with `value` as YYYY-MM-DD), `docket`, `bates`, `citation` (with `§` or `¶`) and `percent`.
  - **Dockets:** federal shapes such as `3:26-cv-04417` anywhere, and any court's format after a label (`Case No.: 11-4604 PSG`, `C.A. No. 5215-VCG`, `Index No. 651234/2021`). `Docket No. 35` cites an entry, not a case, and is left out.
  - **Amounts:** with `$`, and without it in three shapes that cannot be a count: `USD 5,000`, `5,000 dollars`, and `sum of 5,000.00` (cents required).
- Each field has the exact text as it appears in the Markdown, its page, and its line within the page.
- **`verified`:** the same value appears in the page's embedded PDF text, a second, independent reading. Values are compared, not substrings: `$3,000` does not match `$3,000,000` in the embedded text, and `3:26-cv-4417` matches `3:26-cv-04417`. Born-digital filings are mostly verified for free.
- **`check`:** something speaks against the value. `why` says what:
  - `suspect_page`: the page is in `suspect_pages`, failed, or has `missing_text`.
  - `not_in_embedded_text`: the page has embedded text and the value is not in it.
  - `not_in_ocr_text_layer`: the page's embedded text is an invisible OCR layer (see `ocr_text_layer`), and the value is not in it.
  - `confusable_characters`: a letter that looks like a digit (`O`/`0`, `l`/`1`, `S`/`5`, `B`/`8`), such as `3:26-cv-O4417` or `$1,O00`. Such a field has no `value`.
  - `invalid_date`: an impossible date, such as `02/30` or `0Q`.
  - `malformed_amount`: broken digit grouping, such as `$4825,0.00`.
  - `differs_from_usual_docket`: one digit off the docket number the document repeats.
  - `bates_out_of_sequence`: the Bates number breaks the page sequence.
  - `not_in_second_reading`: a scanned page (no embedded text) was read a second time, and that reading does not have the value.
  - `doubtful_in_second_reading`: the second reading has the value, but Vision marks that line as doubtful or offers a different next-best reading.
  - `words_numerals_mismatch`: an amount in words disagrees with its numerals. `Five Thousand Dollars ($3,000.00)` means one side was misread, or the document itself is wrong.
  - `arithmetic_mismatch`: a table's total row disagrees with the sum of the amounts above it. A `Subtotal` row and what follows it, such as tax, are allowed for.
  - `unconfirmed`: with `--verify-fields all`, nothing confirmed the value.
- **Second reading of scanned pages (macOS):** pages without embedded text that have values are read again with Vision's line recognizer at 300 DPI (`second_reader: true` on the page).
  - **Model engines:** when the second reading has the same value, the value is `verified` (`matches_second_reading`). A model and Vision are independent readers.
  - **Apple engine:** agreement verifies nothing, because two Apple readings share blind spots. On one filing both read two overprinted stamps, `04/24/24` and `04/18/24`, as `04/28/24`. A disagreement still turns the value into `check`.
- **`unchecked`:** nothing confirms the value and nothing speaks against it. This typically means a scan with no embedded text. `--verify-fields all` turns these into `check`; use it when nothing may be missed.
- **Values:** amounts include a scale word (`$33.5 million` has the value 33500000), also when the word starts the next line, and a `K`/`M`/`B` suffix (`$1.5B`). Dates are YYYY-MM-DD. A day-first date at the start of a line (`2 March 2023`) is skipped, because there it is a pleading line number before `March 2023`.
- **A value that matches the embedded text is `verified`,** even on a suspect page: two independent readings agree on it.
- **`ocr_text_layer`:** the page's embedded text is drawn invisibly over a scan (text render mode 3, or fully transparent as iOS scanner apps draw it). That is how OCR tools add a text layer, so the text is another OCR's reading, not the document's own characters.
  - **Values:** agreement with it verifies nothing. A disagreement still turns a value into `check`.
  - **Text agreement:** `text_agreement` is `compared_with: ocr_text_layer`, `independent: false`.
  - **Second reading:** the page never gets `second_reading: embedded_text`.
  - On the public filings this applies to a 1985 note and a handwritten dealer form, whose 120 values were verified against their old layers before.
- **`page_check`:** the suspect reasons, `failed: …`, `missing_text`, `missing_number`, `words_numerals_mismatch` or `arithmetic_mismatch`, when the whole page needs a second reading.
  - **`missing_text`:** the other reading (embedded text or second reader) has whole lines the OCR lacks: at least 5 words in lines with at least 4 words, of which fewer than a third of the word pairs appear anywhere in the OCR. They are listed in `text_agreement.missing_lines`.
  - **`missing_number`:** the other reading has a number on a short line that the OCR has nowhere (`30` in `Pay within` / `30` / `days of notice`): a dropped deadline, amount or date. Listed in `text_agreement.missing_numbers`. A pleading's line numbers (five or more lone numbers) are not counted.
  - On the public filings this caught Apple Vision dropping a whole visible paragraph ("ARTICLE NINTH: Execution of Agreement") and a footnote.
  - On embedded text it can also mean text that is not visible on the page, such as text under a drawn redaction box.
  - A born-digital page with `missing_text` gets `second_reading: embedded_text` when its values match, so reading the PDF's own text covers it at no cost.
- **`second_reading`:** how to get the page's second reading, if it needs one.
  - `embedded_text`: a suspect page whose values all match the PDF's own text; read that text instead, at no cost.
  - `azure`: any other page that needs a second reading.
- **`azure_pages`:** the `azure` pages. It is empty when nothing needs a paid second reading.
- **`text_agreement`:** which sentences of the page another reading has word for word, for quoting.
  - `compared_with` is `embedded_text` or `second_reader`.
  - `independent` is true for embedded text, and for Vision against a model engine. Quote as agreed only from independent comparisons, because two Apple readings share blind spots.
  - `unagreed` lists the sentences that do not agree, each with its line within the page's Markdown and the words the other reading lacks entirely.
  - `agreed_spans` and `agreed_words` count what agrees. `summary.agreed_spans` and `summary.unagreed_spans` total the independent comparisons.
  - **What is compared:** letters and digits, case-insensitively, and in order; in a number also a `.` or `,` between digits, a minus sign and `%`, so `$1.00` does not agree with `$100`, nor `-25%` with `25%`. A changed, dropped or doubled word breaks agreement. Each word of the other reading backs one sentence only, so a sentence read twice where the page has it once agrees once; the sentences of one line agree only in their order. Line breaks, hyphenation and pleading line numbers don't, and neither does a margin legend that the PDF text puts between a paragraph's lines.
  - **Edges:** a sentence must also start and end where a sentence or line does in the other reading, so a dropped first or last word is caught. Headers and table cells are matched within longer lines.

Measured on the 20 public court filings (230 pages, Apple engine, eval/PUBLIC_SET.md; binary `87f0ac6`, 2026-10-09). A filing counts as e-filed when every page has embedded text, scanned when none has.

| | `flagged` (default) | `all` |
|---|---|---|
| Pages in `azure_pages` | 57 (25%) | 80 (35%) |
| E-filed filings (146 pages) | 17 | 17 |
| Partly e-filed filings (25 pages) | 2 | 5 |
| Scanned filings, no embedded text (59 pages) | 38 | 58 |
| Values `verified` free against the embedded text | 603 of 860 (70%) | 603 of 860 |
| Values left `unchecked` | 137 | 0 |
| Suspect pages answered by the embedded text instead | 69 | 69 |

11 of the 17 e-filed pages are one 1985 filing whose embedded text is an old OCR layer; the other 133 e-filed pages sent 6. The set is weighted toward scans, so the share of pages sent to Azure depends mostly on how many scans a caseload has.

Real misreads it caught:
- impossible dates (`04/02/2908` for 2008, `81/17/1995`, `02/03/0Q`);
- docket numbers one character off (`3:85-CV-01720-SLMR`);
- `§ 1988` and `§ 77q` read as `$1988` and `$ 77`, which do not match the embedded text.

The embedded text of a scanned PDF may itself be an old OCR layer. A match is still two independent readings that agree. A wrong value that both readings share, or one on a page with no embedded text in `flagged` mode, is not caught.

## Which engine?

| Engine | Best for | Speed (measured) | 300-page estimate | Memory (server peak) |
|---|---|---|---|---|
| apple | General documents, contracts, pleadings, scans, bulk runs. Transcripts work (line numbers re-anchored) but dots-ocr is more accurate | about 3.4-3.5 pages/s at 150 DPI with the Vision limit of 2 (16 pages/s before the limit; see below) | about 85-87 s | `ocr` worker about 178 MB |
| paddleocr-vl | The fastest model engine; text-heavy documents, tables | about 0.19 pages/s (5.3 s/page at 150 DPI) | about 27 minutes | 4.0 GB |
| olmocr | Clean prose and tables in ordinary documents | about 0.06 pages/s (about 16 s/page, from the spike) | about 80 minutes | 11-11.5 GB (5.5 GB at 4bit, estimated) |
| dots-ocr | **Deposition transcripts (most accurate)** and layout-heavy pages | about 0.086 pages/s (11.6 s/page at 100 DPI) | about 58 minutes | 6.8 GB |

Throughput comes from [docs/eval.md](docs/eval.md) on a 32 GB M-series Mac: Apple on a quiet machine, Paddle on a 500-page production, dots on 60 pages, olmOCR from the spike (my eval runs of it were contended). Apple throughput is measured with the Vision concurrency limit of 2 (a crash fix): on a quiet machine a 300-page file took 85-87 s at the 150 DPI default (84-96 s at 200 DPI), about 3x slower than unlimited Vision (29 s). The earlier 1.5x estimate was taken while a model was using the GPU and understated the cost. `OCR_VISION_MAX_CONCURRENT` raises the limit at the risk of start-up crashes. Accuracy on the synthetic eval set (accept seeds; token recall is the primary metric; the transcript line figures marked "after" are from held-out seeds 40-42, see below):

| Engine | Token recall | CER | Bates | Notes |
|---|---|---|---|---|
| apple | 97.4% (no transcripts) | 2.3% | 93/105 pages on pleadings | fax scans 96.3% at the 150 DPI default (88.5% at 200) |
| paddleocr-vl | 99.6% (no transcripts) | 3.3% (pleading CER inflated by margin numbers) | 100% | transcripts 81.5% recall; lines exact 25% before re-anchoring, 94.9% after |
| dots-ocr | 100% (no transcripts and 99.8% on a 60-page production) | 0.6% | 100% | transcripts 100% recall; lines exact 68% before re-anchoring, 99.9% after |
| olmocr | 99.3% (no transcripts) | 0.0% | 10 of 11 pages | transcripts 96.9% recall; lines exact 30% before re-anchoring, 99.9% after |

Guidance:

- **Start with Apple.** It is fast and light, with exact case numbers and amounts on the test fixtures, and does not need a server.
- **Deposition transcripts: use `dots-ocr`, and check `line_numbers.unverified`.** Transcripts number every physical line, wrapped continuations included, and the models renumber sequentially after a wrap. So for every engine (Apple included) the line numbers of transcript pages are re-anchored to the page geometry, never taken from the model. On held-out seeds 40-42, lines exact were dots-ocr 99.9%, olmOCR 99.9%, Apple 97.4% and paddleocr-vl 94.9% (before re-anchoring: 68%, 30%, 38% and 25%). dots-ocr is the most accurate engine for transcripts and `paddleocr-vl` the least. A page whose text could not be matched to the geometry keeps the engine's own numbering and is listed under `line_numbers.unverified` in the front matter; `ocr` prints a note for those pages. Verify page:line citations on them against the PDF.
- **Pleadings with tables on `paddleocr-vl`:** its table crops pull the margin line numbers in as an extra column. Use another engine for pleading-paper tables.

## Limitations

- Transcript line numbers are re-anchored to page geometry for every engine, but not perfectly (97.4% of lines exact for Apple, 94.9% for paddleocr-vl on held-out seeds; 99.9% for dots-ocr and olmOCR). Apple flags transcript pages. Pages listed under `line_numbers.unverified` keep the engine's own numbering; verify page:line citations on them.
- The raw text-layer path (`--use-text-layer always`) has no layout handling. The default `auto` does not correct pages whose text layer is invisible OCR, because that layer is another reading, not the document's characters.
- Pleading margin line numbers (1-2 digits in the left margin) are dropped by Apple Vision, dots and olmOCR but kept by `paddleocr-vl`, where they can be glued to paragraph numbers and land in table crops on pleading paper. Recovering `page:line` citations is not implemented.
- With olmOCR, a rotated page loses its Bates number in the marker, because the Apple side-pass cannot read a sideways page.
- Column detection was tuned on synthetic fixtures; it found about 97.6% of lines on held-out synthetic layouts, and unusual layouts can still be mis-ordered.
- Fixtures and evaluation data are synthetic, not real filings. Apple Bates accuracy on pleadings was 88.6% on the accept seeds.
- Apple Vision crashed intermittently at process start in the evaluation binary (an Apple framework init race); the current build caps Vision at 2 concurrent requests and re-runs a crashed child from its checkpoints.
- Handwriting is not evaluated; there is no trustworthy ground truth for it.
- `suspect_pages` catches gibberish, duplicated passages, prose read as a table and transcript pages out of order, but not wrong-but-plausible numbers, dropped text or readable text in the wrong order. Its thresholds were set on synthetic text and simulated misreads, not yet re-checked on the real filings of eval/PUBLIC_SET.md.
- Model engines are slower than they look: a 300-page scan takes minutes to hours depending on the engine, so use the progress bar ETA and rely on resume.

## Using ocr from an agent (Claude Code skill)

[skills/ocr/SKILL.md](skills/ocr/SKILL.md) is a Claude Code skill. It tells an agent when to reach for `ocr`, how to check setup, which engine to pick, how to read exit codes, front matter and Bates markers, and to keep documents local. Install it for your user account:

    mkdir -p ~/.claude/skills && cp -R skills/ocr ~/.claude/skills/

or for one project only, copy it to `<project>/.claude/skills/ocr/`. The skill does not install the `ocr` binary itself.

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
- [docs/eval.md](docs/eval.md): evaluation method, per-engine results, targets and known gaps.
- [docs/azure-eval.md](docs/azure-eval.md): prompt for an Azure Document Intelligence comparison on another machine.

## License

MIT; see [LICENSE](LICENSE). Model weights are not bundled: `ocr server setup` downloads them from Hugging Face under each model's own license.
