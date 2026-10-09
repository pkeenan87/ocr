---
name: ocr
description: OCR PDFs and images to Markdown locally on a Mac with the `ocr` command-line tool (Apple Vision, PaddleOCR-VL, olmOCR 2, dots.ocr). Use when the user wants text out of a scanned or image-only document, a folder of documents, a deposition transcript, or a legal filing with Bates numbers, and the work must stay on their machine.
---

# ocr: local document OCR to Markdown

`ocr` turns PDFs and images into Markdown on Apple Silicon Macs (macOS 26 or later). Nothing is uploaded. Treat the user's documents as confidential: never send them, or the Markdown made from them, to any web service unless the user asks.

## Before scanning

1. Check that the tool is installed: `command -v ocr && ocr --version`. If it is missing, tell the user and stop; do not try to install it yourself.
2. The default engine, `apple`, needs no setup. For a model engine, run `ocr server setup --model <engine>` once. It downloads several GB, so confirm with the user first.
3. A model engine also needs its server running: `ocr server start --model <engine>`. Only one model server runs at a time. Run `ocr server stop` when you finish, unless the user wants it left running.

## Choosing an engine

| Document | `--model` | Notes |
|---|---|---|
| Most things: contracts, pleadings, letters, scans, big batches | `apple` (default) | About 3 pages/s; 300 pages in about 90 s |
| Deposition transcripts (full-page or 4-up condensed) | `dots-ocr` | Most accurate on transcripts; about 12 s/page |
| Table-heavy documents, or a faster model engine | `paddleocr-vl` | About 5 s/page |
| Clean prose and tables | `olmocr` | Slowest (about 16 s/page), needs 11 GB of memory |

Start with `apple` unless the document is a transcript or the user asks for a model. Before a model run of more than about 50 pages, tell the user the estimated time.

## Scanning

```bash
ocr scan --input-file complaint.pdf                         # writes ./complaint.md
ocr scan --input-file scan.pdf --output-file out/scan.md
ocr scan --input-dir ./case                                 # writes ./case/ocr-output/<stem>.md
ocr scan --input-dir ./case --recursive --file-type pdf
ocr scan --input-file depo.pdf --model dots-ocr
ocr scan --input-file big.pdf --pages 1-50,75
ocr scan --input-dir ./case --dry-run                       # show input -> output mapping only
```

Useful flags:
- `--legal-cleanup` strips running headers and footers that repeat across pages. It changes citable text, so use it only when the user wants that.
- `--dpi 200` can help on clean pleadings; the default is 150.
- `--overwrite` discards earlier output and checkpoints.
- `--quiet` gives less progress output.

Long scans write per-page checkpoints. If a run is interrupted, running the same command again resumes where it stopped. Run long scans in the background and check on them, rather than waiting on one blocking call.

## Exit codes

| Code | Meaning | What to do |
|---|---|---|
| 0 | Success | |
| 1 | Some pages were flagged or failed | Read `flagged_pages` in the front matter and tell the user which pages to check |
| 2 | Setup or usage error, such as an engine that isn't installed | Read the message; usually `ocr server setup` is needed |
| 3 | Model server not running or crashed | `ocr server start --model <engine>`, then re-run the same command to resume |

## Reading the output

Each `.md` file starts with YAML front matter. Then every page begins with a marker: `<!-- page 12 -->`, or `<!-- page 12 · Bates ABC0000123 -->` when a Bates number was found. Cite pages and Bates numbers from these markers.

Front matter fields to watch:
- `flagged_pages`: pages to check by hand.
- `fallback_pages`: the model failed on these pages, and Apple Vision text was used instead.
- `line_numbers.unverified`: transcript pages whose line numbers could not be checked against the page geometry.
- `column_split_pages`: pages read column by column.

OCR text can be wrong in ways that look plausible: digits in amounts, ¶ read as 9 or T, § read as S or $. When an exact figure, quotation or citation matters, say that it came from OCR and suggest checking it against the PDF page.
