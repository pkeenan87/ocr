# Residual risk of `ocr --verify-json` with Azure on the flagged pages: results

Plan and prompt: [azure-verify-eval.md](azure-verify-eval.md). Script: `eval/azure_verify_eval.py`. Raw outputs: [azure-verify-eval-results/](azure-verify-eval-results/) (`<run>-<route>.json`; `apple-all.json` is run `apple`, route `all`).

## 1. Environment

- 2026-10-09, macOS 27.0.1, Python 3.12.13 (repo venv, `eval/requirements.txt`), master at `0a90ec4`.
- **No Azure call.** The 20 responses from the 2026-10-09 comparison (`eval/out/public/azure-di/<id>.azure.json`) were still on this machine. All 20 passed the check: no failed `status`, `apiVersion` `2024-11-30`, `modelId` `prebuilt-layout`, pages 1 to `n_pages_used` present. Cost: none.
- The script ran unchanged. `azure_pages()` needed no fix.

## 2. Results

Graded: 139 pages with exact embedded text (no invisible OCR layer) out of 230, holding 503 values (498 right, 5 wrong by the truth run). The value columns are the same in every run because the blind reports send every graded page with an unverified value to Azure.

| Run, route | Pages to Azure | Values relied on | Relied on but wrong: raw / real | Wrong stopped (disputed / person) | Right values disputed / to a person | Sentences confirmed (not in embedded text) / unconfirmed | Confirmed sentences really wrong |
|---|---|---|---|---|---|---|---|
| `apple-all`, report | 222 (96.5%) | 473 of 503 (94.0%) | 2 / **0** | 3 of 5 (3 / 0) | 27 / 0 | 2,155 (113) / 395 | 21 (+28 on flagged pages) |
| `apple-all`, all | 230 (100%) | 473 | 2 / **0** | 3 (3 / 0) | 27 / 0 | 2,233 (117) / 412 | 21 (+28) |
| `apple`, report | 128 (55.7%) | 209 (41.6%) | 2 / **0** | 3 (3 / 0) | 18 / 273 | 1,297 (83) / 345 | 21 (+28) |
| `apple`, all | 230 (100%) | 473 | 2 / **0** | 3 (3 / 0) | 27 / 0 | 2,233 (117) / 412 | 21 (+28) |

- Nothing is `verified` in any blind run: as designed, Apple alone confirms nothing without the embedded text. Every relied-on value is "confirmed by Azure".
- `apple`/`all` equals `apple-all`/`all`: the blind fields are the same in both modes. Only the routing differs. With `flagged`, 273 right values never reach Azure and go to a person, which is why the skill uses `all`.
- "+28 on flagged pages": spliced sentences on removed-1 pages that are in `suspect_pages`. The skill already says not to quote from those pages; the script doesn't model that rule (see 4.2).

## 3. Azure alone (correlated errors)

- Azure has **94.6%** of the right values (471 of 498).
  - The 27 it lacks: 8 are the Jane Street header date, where Azure reads the overprinted stamp `04/24/24` and Apple the original `04/18/24` (both are printed, one over the other).
  - 15 are on Knight-Avery: Azure leaves out the CM/ECF header stamp with the docket and filed date.
  - 2 are Garland `¶ 60` and `¶ 46`, which Azure reads as `160` and `146`.
  - All 27 end as disputed, not as wrong values relied on.
- Azure repeats **2 of 5** raw wrong values, but **0 of 3 real ones**. Both repeats are `5-16-cv1128` (section 4), which is bad truth.
- The 3 real wrong values are: `5:15CV-01102-JGB`, twice (Apple's field is cut short; the page has `5:15-CV-01102-JGB-SP`); and removed-1 p12 `201%`, from a header stamp printed over `2019`. Azure disagrees on all three.

That is no correlated error among graded values, but only 3 real wrong values were there to repeat. The correlated errors the pages do show are outside the value grading:
- The same `¶` misread as a digit by both readers, differently. Jane Street p7 `¶¶ 8, 152`: Apple `19| 8`, Azure `11 8`. Jane Street p8 `Id. ¶ 8`: Apple `98`, Azure `18`. Ironworkers p2 `¶¶ 12-16`: Apple `91 12-16`. No field is extracted from them, so no value is relied on. A word-for-word comparison would see the readers disagree.
- Both readers reading a two-column transcript across the gutter in the same order (4.2).

## 4. Every reviewed example

All pages rendered at 200 DPI to `eval/out/azure-verify/pages/` (not committed) and inspected.

### 4.1 Values: "confirmed but wrong" and "Azure repeats a wrong value"

There are no "verified but wrong" items in any run. The same two items appear in all four runs:

| Document, page | Read as | The page says | Class |
|---|---|---|---|
| cmpl-ab-sanbernardino p15, line 9 | `5-16-cv1128` | `5-16-cv-` / `1128` across a line break | **bad truth** |
| cmpl-ab-sanbernardino p15, line 13 | `5-16-cv1128` (Apple's Markdown repeats the paragraph) | same | **bad truth** |

Apple joined the line and dropped the hyphen, so the letters and digits are right. The skill compares dockets "as their letters and digits", so confirming it is correct. The truth run called it wrong because the PDF codes the line-end hyphen as U+FFFE (`5-16-cv￾1128`). The repeated paragraph is an Apple reading-order defect, not a value error.

**Recount: 0 real errors among the 473 (or 209) values relied on.** No real error came from stamps, handwriting or overprinting among relied-on values. The overprinted stamps that fooled Apple were all stopped:
- Jane Street, Apple's `04/18/24` against Azure's `04/24/24`. On pages 6 and 10, which have no embedded text and are not graded, Apple reads `04/28/24` and Azure `04/24/24`, so those are disputed too.
- removed-1 p12 `201%`.

The known Jane Street failure, both Apple readings agreeing on `04/28/24`, is stopped by Azure here: Azure reads the top stamp.

### 4.2 Sentences: "Azure confirms, not in embedded text"

I reviewed by hand the first 25 of each run (45 distinct sentences), plus every sentence of the full lists (113 / 117 / 83) that an automatic sort could not explain.

| Class | `apple-all` report (113) | What it is |
|---|---|---|
| bad truth: running header or footer in another place | ~18 | `Page 7 of 18 LLCs' funds into one company:`, `... claims COMPLAINT FOR DAMAGES`. The words are on the page; the embedded text has the header elsewhere. |
| bad truth: line-end hyphen coded U+FFFE in the PDF | ~25 | `invi-tation`, `harm-ing`, `Millen-nium`, Ironworkers `03:11-633-AC`. `tokens()` does not join U+FFFE, so the embedded text splits the word. Apple drops genuine compound hyphens at line ends (`thirtysix`, `cofounder`, `RollUp`, `successorin-interest`, `merriamwebster`), a minor wording defect; Azure keeps them. |
| bad truth: embedded text is an older OCR | 7 | ctr-marital-settlement is a phone scan ("Created with Scanner Pro") whose text layer is an old OCR (`Agreenent`, `aftervwiferefinances`). `ocr_text_layer` is false for all 15 pages, so it is graded as born-digital. Apple's and Azure's sentences match the page. |
| matching artifact: the 1-2 digit skip hides a real `¶` misread | 3+ | Jane Street p7 (`19| 8` against Azure `11 8`), p8 (`Id. 98.` against `Id. 18.`), Ironworkers p2 (`at 91 12-16`, `at 9 17`). The readers disagree, so a word-for-word comparison would not confirm them. |
| **real error: columns spliced, page not flagged** | **21** | depo-removed-1 p14, a two-column condensed transcript that is not in `column_split_pages` or `suspect_pages`, with an empty `page_check`. Apple reads across the gutter (`A Yes. 19 A Yes, I am currently employed.`, `Q Then you knew coming up that they were going to 14 Q Okay.`). Azure's lines interleave the same way, so it confirms them. |
| real error, stopped by `suspect_pages` | 28 (+14 on other suspect pages) | removed-1 pp. 2-13, 15, the same splicing (`What is her name? to kick me under the table; but try to let 13 A Is that relevant?`). These pages are flagged, and the skill says not to quote from them. |

The `~` counts come from the automatic sort, whose classes overlap at the margins. The real-error counts are exact.

## 5. Plain terms

- **Values:** of the values the workflow lets a tool rely on, **0 in 1,000** were really wrong here (0 of 473; raw grading 2 of 473, or 4 in 1,000). This rests on very few examples: the graded pages held 3 real wrong values in all, and Azure repeated none. With 0 of 473, a 95% upper bound is about 6 in 1,000, and that is for clean, mostly born-digital filings.
- **Sentences:** of the sentences Azure confirms, about **10 in 1,000** are really wrong (21 of 2,155; 16 in 1,000 with `flagged` routing). All 21 come from one page: a two-column transcript page that `ocr` did not flag, where both readers read across the columns. If a tool ignored `suspect_pages`, it would be 23 in 1,000 (49 of 2,155). The raw grading says 52 in 1,000; most of that is bad truth.
- The main residual risks found are therefore reading order on multi-column pages and `¶` read as a digit, not digit misreads in values. Both readers make the first mistake the same way. For the second they disagree, so it is caught only if the workflow compares the sentence word for word, including 1-2 digit tokens.

## 6. Caveats

- The 20 filings are mostly born-digital with clean scans; only 139 pages could be graded, holding 5 graded wrong values (the Mac's `verify_eval.py` saw 7 on 167 pages, which include pages this script excludes).
- The token match is looser than reconciling by value: it ignores punctuation and allows 3 characters between characters, and the sentence match skips any 1-2 digit token, which hid the `¶` misreads.
- Sentence grading depends on reading order: headers, footnotes and columns are placed differently by the OCR, Azure and the embedded text.
- `ocr_text_layer` missed ctr-marital-settlement, so 15 pages (9 values) were graded against an old OCR.
- The blind Markdown of depo-removed-1 lists `embedded_text_fixes`, so the blind scan still used the PDF's text for some characters. The blind runs may be slightly better than a true scan.
