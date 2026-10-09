# Azure Document Intelligence comparison: results (2026-10-09)

Run per [docs/azure-eval.md](azure-eval.md). This is a reference only: `ocr` does not call Azure. The glue code (submit/poll/cache, page split, Bates, scoring) lived in `eval/out/azure/tools/` (gitignored) and is not committed. Score summary: [azure-eval-results/azure.json](azure-eval-results/azure.json).

## 1. Environment

| | |
|---|---|
| Date | 2026-10-09 |
| Model / API | `prebuilt-layout`, `api-version=2024-11-30` (v4.0 GA; re-checked on the day, still the newest GA), `outputContentFormat=markdown`, `stringIndexType=unicodeCodePoint`, no `features` add-ons. `_overload=analyzeDocument` was accepted. |
| Region | not recorded (user's choice); endpoint never recorded |
| Pricing tier | S0 (user-confirmed; every document returned all requested pages, so not F0) |
| Client | macOS 27.0.1, Python 3.12.13 (uv-managed venv) |
| Fixture rendering | reportlab 5.0.1, pypdfium2 5.14.0, Pillow 12.3.0 (numpy 2.5.3, rapidfuzz 3.14.6): identical to `eval/requirements.txt` |
| Pages analysed | **336**: 46 synthetic (seeds 10–12, five families) + 60 production (seed 10, `pages=1-60`) + 230 public (20 filings). 36 requests, all `succeeded`, none retried. |
| Cost estimate | about **USD 3.36** at the S0 "Pre-built Pages" list price of USD 10.00 per 1,000 (Retail Prices API, eastus, as checked on 2026-10-08). The re-check on the day through the Retail Prices API was not run (the tool call was declined); the user approved the run at that price. |

## 2. Output handling

Each page's Markdown is `analyzeResult.content` sliced by `pages[].spans` (not by `<!-- PageBreak -->`). Checks on all 36 responses: no overlapping spans, no non-furniture content outside page spans, and the PageBreak count equals pages − 1 everywhere.

Written as `<!-- page N -->` / `<!-- page N · Bates X -->` markers with YAML front matter, UTF-8, `\n`.

- **Stripped** (after reading Bates from them): `<!-- PageHeader="..." -->`, `<!-- PageFooter="..." -->`, `<!-- PageNumber="..." -->`, `<!-- PageBreak -->`.
- **Kept as is:** HTML tables, headings, `<figure>`/`<figcaption>`, ☒/☐, and any header/footer text Azure left as an ordinary paragraph.
- **Bates rule** (approximates the Mac's `BATES_ZONE = 0.08`): candidates are the page's `lines[]` whose top edge is ≥ 0.92 × page height, plus paragraphs with role `pageFooter` or `pageNumber` on that page, taken top to bottom; the first match of `\b[A-Z]{2,}[-_ ]?\d{5,}\b` wins. Nothing else on the page is searched.
- Why stripping is neutral: the scorer drops all HTML comments and removes ground-truth furniture from the output before comparing, as for the Mac engines. **Exception found:** see the pleading caveat in section 6.

Scoring: `eval/score.py`, `--strip-line-numbers` on pleading only, furniture ignored, production scored against the first 60 ground-truth pages. Seeds pooled as in `run.py`'s `aggregate()`.

## 3. Synthetic results

Seeds: **Azure, dots-ocr and olmOCR used seeds 10–12; paddleocr-vl used 10–19; Apple used 10–39 at 200 DPI.** Production: Azure, dots and olmOCR on pages 1–60 of seed 10; Apple and Paddle on all 500 pages. Three seeds is 6–14 pages per family, so one page moves a number by whole points. Mac numbers are from [docs/eval.md](eval.md).

**Transcript lines compare raw against raw.** The Mac numbers are before re-anchoring (`src/anchor.rs`); Azure gets no re-anchoring. After re-anchoring, for context only: dots 99.9%, olmOCR 99.9%, Apple 97.4%, Paddle 94.9% (held-out seeds 40–42).

### pleading

| Engine | Seeds | Pages | CER | Token recall | Bates |
|---|---|---|---|---|---|
| **azure-di** | 10–12 | 11 | 0.6% | 100% | 100% (11/11) |
| apple (200 DPI) | 10–39 | 105 | 1.7% | 97.9% | 88.6% |
| paddleocr-vl | 10–19 | 36 | 13.3% (margin-number artifact) | 98.7% | 100% |
| dots-ocr | 10–12 | 11 | 0.0% | 100% | 100% |
| olmocr | 10–12 | 11 | 0.0% | 99.5% | 90.9% |

The whole 0.6% is page 1 of seeds 10 and 11. Azure tags the top-left attorney block ("PATRICIA LAWSON (SBN 303667) DONOVAN & DONOVAN LLP") as `pageHeader`, and the format contract strips it. With that header text kept, Azure's pleading CER is 0.00%.

### contract

| Engine | Seeds | Pages | CER | Token recall |
|---|---|---|---|---|
| **azure-di** | 10–12 | 14 | 0.0% | 100% |
| apple (200 DPI) | 10–39 | 153 | 2.3% | 96.9% |
| paddleocr-vl | 10–19 | 48 | 0.1% | 100% |
| dots-ocr | 10–12 | 14 | 0.2% | 100% |
| olmocr | 10–12 | 14 | 0.0% | 100% |

### transcript4up

| Engine | Seeds | Pages | CER | Token recall | Transcript lines (raw) |
|---|---|---|---|---|---|
| **azure-di** | 10–12 | 9 | 1.0% | 84.5% | **80.0%** |
| apple (200 DPI) | 10–39 | 78 | 63.0% | 83.4% | 38.3% |
| paddleocr-vl | 10–19 | 25 | 51.1% | 81.5% | 25.2% |
| dots-ocr | 10–12 | 9 | 1.2% | 100% | 68.1% |
| olmocr | 10–12 | 9 | 11.2% | 96.9% | 30.4% |

Azure reads the 4-up quadrants in order with the line numbers inline, so raw line accuracy is the best of any engine. The misses are wrapped continuations, where the next line number lands on its own line (`7` then the text). **All 20 missed tokens (129 − 109) are dollar amounts:** Azure inserts a space after the thousands comma (`$288, 760.00` for `$288,760.00`) on every transcript page. The digits are right, but exact search for the amount fails. This did not happen in the contract, table or production families.

### fax

| Engine | Seeds | Pages | CER | Token recall |
|---|---|---|---|---|
| **azure-di** | 10–12 | 6 | 2.2% | 100% |
| apple (200 DPI) | 10–39 | 60 | 7.7% | 88.5% |
| paddleocr-vl | 10–19 | 20 | 0.0% | 100% |
| dots-ocr | 10–12 | 6 | 6.3% | 100% |
| olmocr | 10–12 | 6 | 0.0% | 100% |

The CER comes from the fax transmission header line (`TEL 532-437-1466 PAGE 02/02`), which Azure keeps as text. It is not in the reference, and the scorer's furniture list does not match it. The body text was exact.

### table

| Engine | Seeds | Pages | CER | Token recall |
|---|---|---|---|---|
| **azure-di** | 10–12 | 6 | 0.0% | 100% |
| apple (200 DPI) | 10–39 | 60 | 0.3% | 98.6% |
| paddleocr-vl | 10–19 | 20 | 0.0% | 100% |
| dots-ocr | 10–12 | 6 | 0.1% | 100% |
| olmocr | 10–12 | 6 | 0.1% | 98.9% |

### production

| Engine | Pages | CER | Token recall | Bates |
|---|---|---|---|---|
| **azure-di** | 1–60 | 0.0% | 100% | 100% (60/60) |
| apple (200 DPI) | all 500 | 1.1% | 96.1% | 99.4% |
| paddleocr-vl | all 500 | 0.0% | 100% | 99.4% |
| dots-ocr | 1–60 | 0.6% | 99.8% | 100% |
| olmocr | 1–60 | 0.1% | 100% | 100% |

### Aggregate (without transcripts and production, as the Mac aggregates are shown)

`eval/agg.py eval/out/azure/azure.json`:

```
azure-di: fams=['pleading', 'contract', 'fax', 'table'] CER=0.0034 tokrecall=1.0000 bates=11/11 pages=37
```

Apple's equivalent (30 seeds, 378 pages) is CER 2.32%, token recall 97.4%, Bates 93/105.

## 4. Public set

**Judge skipped: no `AI_GATEWAY_API_KEY`** (not in the environment or `.env`). The 20 outputs are in `eval/out/public/azure-di/<id>.md` (gitignored), ready for `eval/public_compare.py --engines azure-di` once a key is available.

One check without the judge: the legal-symbol count from `eval/PUBLIC_SET.md`, against the born-digital text layers (pages with more than 200 characters of text):

| Symbol | Azure | Apple (PUBLIC_SET.md) |
|---|---|---|
| § | **82 / 82** | 70 / 82 |
| ¶ | **0 / 48** | 0 / 48 |

Azure, like Apple, never outputs ¶. It usually reads it as `1` and glues it to the paragraph number: "Compl. ¶ 35" comes out as `Compl. 135`, and "Id. ¶ 18" as `Id. 118` (ord-garland-dunkin). That is a silent wrong number, worse for citation search than Apple's `9`.

## 5. Throughput (cloud latency, not comparable to the local Mac figures)

Wall time runs from submit to `succeeded`. It includes upload, queueing and polling (at the `Retry-After` interval). Requests were sent one at a time.

| Set | Documents | Pages | Total wall s | Per document (min–max) | Pages/s |
|---|---|---|---|---|---|
| synthetic (2–5 pages each) | 15 | 46 | 133.9 | 6.0–13.7 s | 0.34 |
| production pages 1–60 | 1 | 60 | 16.6 | – | 3.6 |
| public (4–15 pages each) | 20 | 230 | 227.4 | 7.2–15.3 s | 1.0 |

Per-document latency is about 6–16 s almost regardless of size: a 60-page request took 16.6 s. Throughput is therefore governed by fixed latency, and parallel requests would raise it.

## 6. Caveats

- The fixtures are synthetic and template-based (docs/eval.md); real filings are messier. The public set was not judged.
- The fixture rendering versions match `eval/requirements.txt` exactly, so these are the same PDFs the Mac scored.
- 3 seeds, 6–14 pages per family: indicative only, as for dots and olmOCR.
- The Bates rule is approximated from Azure's line geometry and footer roles. Azure's Bates comes from Azure itself, whereas the Mac model engines take it from the Apple side-pass.
- **Stripping PageHeader is not always neutral.** On pleading page 1, Azure classifies the attorney caption block as `pageHeader`. It is page content, not ground-truth furniture, so the contract's stripping costs Azure all of its pleading CER (0.6% vs 0.0% with it kept). Token recall and Bates are unaffected.
- The transcript dollar-amount spacing (`$288, 760.00`) is Azure output, not a scorer artifact. It costs 15.5 points of transcript token recall.
- The fax CER is the transmission header kept as text (section 3).
- Format and span checks: every page has a marker, `split_pages` matches the ground-truth page count for every synthetic file, and there were no span overlaps, orphaned content or PageBreak mismatches.
- Run on the user's Mac rather than a separate work machine. Only the synthetic fixtures and the 20 public filings were sent to Azure.
