---
name: ocr
description: OCR PDFs and images to Markdown locally on a Mac with the `ocr` command-line tool (Apple Vision, PaddleOCR-VL, olmOCR 2, dots.ocr), and check the values and quotes in them before anyone relies on them. Use when the user wants text out of a scanned or image-only document, a folder of documents, a deposition transcript, or a legal filing with Bates numbers, and when amounts, dates, docket numbers, citations or quotations from such documents will be relied on. It is the entry point for the verification workflow with a second OCR service such as Azure Document Intelligence.
---

# ocr: local document OCR to Markdown

`ocr` turns PDFs and images into Markdown on Apple Silicon Macs (macOS 26 or later). Nothing is uploaded. Treat the user's documents as confidential: never send them, or the Markdown made from them, to any web service, unless the user asks or has set up a second-OCR tool for exactly this (step 3 of the verification workflow below).

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

## Values and quotes that will be relied on: the verification workflow

Use this workflow whenever the user will rely on figures, dates, docket or Bates numbers, citations or quotations: legal work by default. OCR misreads look plausible (`$3,000` for `$8,000`), and a wrong value is worse than a missing one. A value counts as confirmed only when two independent readings agree on it.

### 1. Scan with the verification report

```bash
ocr scan --input-file filing.pdf --verify-json --verify-fields all
```

Always use `--verify-fields all` for this workflow. The default, `flagged`, leaves values on scanned pages `unchecked` and keeps their pages out of `azure_pages`, so they would never get a second reading. The report is written to `<stem>.verify.json` next to the Markdown.

### 2. Read the report

- `summary`: how many values are `verified`, `check` and `unchecked`.
- `azure_pages`: the pages that need a second reading, in Azure's `pages` syntax (`3,7-9`). It is empty when the PDF's own text already confirmed everything.
- Per page: `fields` (each with `kind`, `text`, `value`, `line`, `status`, `why`), `page_check`, `second_reading`, `text_agreement`, `ocr_text_layer`.
- `verified` values were matched by an independent reading: the PDF's own characters, or Vision against a model engine. Nothing else is `verified`.

### 3. Get the second reading, only for `azure_pages`

- **If a second OCR tool is available** (an Azure Document Intelligence tool, for example), send it only the pages listed in `azure_pages`, using its `pages` parameter. Never send the whole document when only some pages are listed. This is what keeps the cost down.
- **Confidentiality:** sending pages to a cloud service publishes them to that service. Do it only through a tool the user or their organisation has set up for this purpose. If you cannot tell whether these documents may go to it, ask first.
- **If no second reading is available**, nothing in step 4 can be confirmed. Tell the user which pages need checking against the PDF (`azure_pages`), and treat every `check` and `unchecked` value as unconfirmed.
- Pages whose `second_reading` is `embedded_text` need no service: their values already match the PDF's own text. Read the page's text from the PDF instead of the OCR if you need its wording.

### 4. Reconcile each unconfirmed value with the second reading

For every `check` or `unchecked` field on a page that was re-read, find the same value in the second reading of that page, and compare it **by value, not as text**:
- amounts in cents, so `$3,000` is not `$3,000,000`;
- dates as YYYY-MM-DD;
- docket and Bates numbers as their letters and digits;
- citations by section or paragraph number **and their sign**: `¶ 60` and `160` are different values, and so are `§ 5` and `§§ 1301`. Both readers tested misread `¶` as `1`, `9` or nothing, each in its own way, so a citation whose sign is missing or different in either reading is a disagreement, never a match;
- an amount with its scale word, which may be on the next line or past a footer (`$45` / `million`): compare the whole amount.

Then:
- **Same value:** confirmed by two independent readings. You may state it as fact, with its page.
- **Different value:** a disagreement. Never pick one. Report both readings and the page, and say the PDF page must be checked.
- **Not found in the second reading:** still unconfirmed.
- **Special reasons win.** A value with `words_numerals_mismatch` or `arithmetic_mismatch` stays in dispute even if the second reading agrees, because the document contradicts itself (`Five Thousand Dollars ($3,000.00)`). Say so.
- **Stamps, handwriting and margin notes:** say where the value came from even when it is confirmed. Overprinted or doubled stamps fooled every pair of image readers tested (`04/24/24` over `04/18/24` read as `04/28/24` by all of them).

### 5. What you may state, quote and compute

- **State or compute only with values that are `verified`, or confirmed in step 4.** Always give the page, and the Bates number if there is one.
- **Quote word for word only agreed sentences.** A sentence agrees when it is not in its page's `text_agreement.unagreed` and `text_agreement.independent` is true. A sentence that the second reading in step 3 has word for word also agrees. Otherwise paraphrase, and say the exact wording is unconfirmed.
- **A word broken at the foot of a page** (`non-`) continues on the next page, past the footer and header (`voting shares`). Never join it to the footer (`nonCOMPLAINT`).
- **`missing_text` or `missing_number` in `page_check`:** the OCR lacks whole lines, or a number, that another reading has (`text_agreement.missing_lines`, `missing_numbers`): a dropped deadline or amount can change what a sentence says. Never say a page does not mention something from its OCR alone. Use the second reading's text, or check the PDF.
- **`ocr_text_layer: true`:** the page is a scan carrying an older OCR's invisible text layer. That layer counts as one more reading, not as the document's own characters, so agreement with it confirms nothing. Treat the page as a scan.
- **`check` and `unchecked` values that step 4 did not confirm are unconfirmed.** Never present them as fact.
- **Never fill a gap from memory or by inference.** If a value cannot be confirmed, say that it needs checking against page N of the PDF.

### 6. Tell the user where things stand

End with a short account:
- how many values were confirmed, and by what (the PDF's own text, the second reading);
- which values disagree, with both readings and their pages;
- which pages still need a person to check them, and why.

Name pages and Bates numbers so the user can find them.

## Reading the output

Each `.md` file starts with YAML front matter. Then every page begins with a marker: `<!-- page 12 -->`, or `<!-- page 12 · Bates ABC0000123 -->` when a Bates number was found. Cite pages and Bates numbers from these markers.

Front matter fields to watch:
- `flagged_pages`: pages to check by hand.
- `suspect_pages`: `{page: [reasons]}` for pages whose text looks wrong although OCR succeeded (`garbled`, `misreads`, `junk_lines`, `duplicated`, `pleading_table`, `transcript_table`, `page_order`, `mixed_case`). Tell the user which pages and why, and do not quote or cite from them without checking the PDF page.
- `symbol_fixes`: pages where `¶`/`§` were restored in Apple Vision citations (`Compl. 9 60` became `Compl. ¶ 60`); `--verbose` prints each rewrite.
- `fallback_pages`: the model failed on these pages, and Apple Vision text was used instead.
- `line_numbers.unverified`: transcript pages whose line numbers could not be checked against the page geometry.
- `column_split_pages`: pages read column by column.

OCR text can be wrong in ways that look plausible, and these are not caught by `suspect_pages`: digits in amounts, a dropped sentence, ¶ or § misread outside the citation shapes that `ocr` repairs. When an exact figure, quotation or citation matters, say that it came from OCR and suggest checking it against the PDF page.
