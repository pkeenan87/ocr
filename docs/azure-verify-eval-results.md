# Residual risk of `ocr --verify-json` with Azure on the flagged pages: results

Plan and prompt: [azure-verify-eval.md](azure-verify-eval.md). Script: `eval/azure_verify_eval.py`. Raw outputs: [azure-verify-eval-results/](azure-verify-eval-results/) (`<run>-<route>.json`; `apple-all.json` is run `apple`, route `all`).

## 1. Environment

- 2026-10-09, macOS 27.0.1, Python 3.12.13 (repo venv, `eval/requirements.txt`), master at `0a90ec4`.
- **Mac runs regenerated with the latest build.** The committed `eval/verify_bundle/` was made with a `target/release/ocr` built at 10:58, before PR 14's source change (`src/verify.rs`, `src/run.rs`) was committed at 12:26. Its README also says macOS 26; this machine is now on 27.0.1.
  - I rebuilt `ocr` from master (`cargo build --release`) and re-ran `eval/verify_eval.py` for `--verify-fields all` and `flagged`, into `eval/out/verify-latest/<run>/`, with the same layout as the bundle.
  - The results below use those runs (`--bundle eval/out/verify-latest/<run>`).
  - The new build's text is closer to the embedded text on 13 of the 14 documents with enough embedded text to compare (Uber-O'Connor is 2 points worse), by up to 12 points (Knight-Avery 31% to 20%, Ironworkers 32% to 22%, Nissan 40% to 29%, word-level distance).
  - The committed bundle has not been replaced yet.
- **No Azure call.** The 20 responses from the 2026-10-09 comparison (`eval/out/public/azure-di/`) were reused. All passed the check: no failed `status`, `apiVersion` `2024-11-30`, `modelId` `prebuilt-layout`, pages 1 to `n_pages_used` present. Cost: none.
- The script ran unchanged. `azure_pages()` needed no fix.

On the Mac side, the regenerated runs give 167 pages with embedded text and 15 wrong values among the graded ones (the old bundle: 7). With `all`, all 15 are `check` in the blind run; with `flagged`, 8. Nothing is `verified`.

## 2. Results

Graded: 139 pages with exact embedded text (no invisible OCR layer) out of 230, holding 528 values (513 right, 15 wrong by the truth run).

| Run, route | Pages to Azure | Values relied on | Relied on but wrong: raw / real | Wrong stopped (disputed / person) | Right values disputed / to a person | Sentences confirmed (not in embedded text) / unconfirmed | Confirmed sentences really wrong |
|---|---|---|---|---|---|---|---|
| `apple-all`, report | 218 (94.8%) | 499 of 528 (94.5%) | 3 / **1** | 12 of 15 (12 / 0) | 17 / 0 | 2,176 (58) / 380 | 1 (+9 on suspect pages) |
| `apple-all`, all | 230 (100%) | 499 | 3 / **1** | 12 (12 / 0) | 17 / 0 | 2,355 (63) / 408 | 1 (+9) |
| `apple`, report | 98 (42.6%) | 163 (30.9%) | 3 / **1** | 12 (8 / 4) | 9 / 344 | 1,062 (25) / 313 | 0 (+8) |
| `apple`, all | 230 (100%) | 499 | 3 / **1** | 12 (12 / 0) | 17 / 0 | 2,355 (63) / 408 | 1 (+9) |

- Nothing is `verified` in any blind run: Apple alone confirms nothing without the embedded text, as designed. Every relied-on value is "confirmed by Azure".
- `apple`/`all` equals `apple-all`/`all`: the blind fields are the same in both modes. Only the routing differs. With `flagged`, Azure sees 98 pages, and 344 right values and 4 wrong ones go to a person. `all` sends 218 pages and leaves nothing to a person.
- "+9 on suspect pages": spliced transcript sentences on pages in `suspect_pages`. The skill says not to quote from those pages, but the script doesn't model that rule (see 4.2).

## 3. Azure alone (correlated errors)

- Azure has **96.7%** of the right values (496 of 513).
  - The 17 it lacks: 12 are Knight-Avery, where Azure leaves out the CM/ECF header stamp with the docket and filed date.
  - 5 are `¶` citations that Azure reads as `1` (Garland `¶ 60` as `160`, `¶ 46` as `146`; Ironworkers `¶ 33`, `¶ 35`, `¶ 30`).
  - All 17 end as disputed.
- Azure repeats **3 of 15** raw wrong values. Of the 14 real ones, it repeats the characters of **1**: `$45`, which is an `ocr` extraction error, not a misreading (section 4). Of the 13 real misreadings, Azure repeats **0**.
- The correlated errors the pages do show are outside the graded values:
  - `¶` read as a digit, or dropped, by both readers, differently. For example Jane Street p7 `¶¶ 8, 152`: Apple `1 8, 152`.
  - The page-end hyphen joined to the footer (4.2).
  - Columns spliced on transcript pages, now confined to pages `ocr` flags.

## 4. Every reviewed example

All pages rendered at 200 DPI to `eval/out/azure-verify/pages/` (not committed) and inspected.

### 4.1 Values: "confirmed but wrong" and "Azure repeats a wrong value"

There are no "verified but wrong" items. The same three items appear in all four runs:

| Document, page | Read as | The page says | Class |
|---|---|---|---|
| cmpl-irhythm p8, line 15 | `$45` (value 45) | `$45` / `million.` across a line break | **real error** |
| cmpl-irhythm p9, line 17 | `§ 5` | `42 U.S.C. §§ 1301` | **matching artifact** over a real Apple misread |
| cmpl-ab-sanbernardino p15, line 7 | `5-16-cv1128` | `5-16-cv-` / `1128` across a line break | **bad truth** |

- **`$45`:** the value is "$45 million", and `ocr` extracts the amount without the next line's `million`. Its other field on the same page, `$45 Million` (footnote 8), is right. Azure has the same `$45` at the end of a line, so the token match confirms it. A reconciler that reads Azure's amount across the line break would get $45,000,000 and dispute it; one that matches `$45` would not. Counted as real.
- **`§ 5`:** Apple read the second `§` of `§§ 1301` as `5`. The match ignores `§` and found a stray `5` elsewhere on the page. Azure's citation is `§§ 1301`, so reconciling by section number would not confirm it.
- **`5-16-cv1128`:** Apple joined the line and dropped the hyphen. The letters and digits are right, and the skill compares dockets that way. The truth run missed it because the PDF codes the line-end hyphen as U+FFFE.

**Recount: 1 real error among the 499 (or 163) values relied on.** It does not come from a stamp, handwriting or overprinting.

The wrong values stopped include every stamp case:
- **Jane Street:** the known overprinted stamp. Apple now reads `04/28/24` on all eight graded pages (`04/24/24` printed over `04/18/24`), and Azure reads `04/24/24`. All 8 are disputed.
- **Knight-Avery:** stamps over the transcript, `08/03/24` for `08/01/24` and `2:23-cv-00386-NJ` for `-00886-`. All 3 are disputed.
- **San Bernardino:** `5:15CV-01102-JGB`, a field cut short (the page has `5:15-CV-01102-JGB-SP`). Disputed.

### 4.2 Sentences: "Azure confirms, not in embedded text"

I reviewed by hand the first 25 of each run (43 distinct sentences), plus every sentence of the full lists (58 / 63 / 25) that an automatic sort could not explain.

| Class | `apple-all` report (58) | What it is |
|---|---|---|
| bad truth: line-end hyphen coded U+FFFE in the PDF | ~27 | `invi-tation`, `harm-ing`, `Millen-nium`, `cv-01023`. `tokens()` does not join U+FFFE. Apple drops genuine compound hyphens at line ends (`thirtysix`, `cofounder`, `RollUp`, `coworkers`), a minor wording defect; Azure keeps them. |
| bad truth: running header, footer or caption in another place | ~14 | `Page 5 of 18 RELEVANT ENTITIES`, `... COMPLAINT FOR DAMAGES`, the Kuzdas caption (`(305) 982-6300 UNITED STATES DISTRICT COURT`), Uber `Page 45 Page 47 ...` |
| bad truth: embedded text is an older OCR | 4 | ctr-marital-settlement, a phone scan ("Created with Scanner Pro") with an old OCR layer (`Agreenent`, `aftervwiferefinances`) that `ocr_text_layer` misses. Checked on the page: Apple and Azure are right. |
| matching artifact: the 1-2 digit skip hides a `¶` misread | ~5 | Jane Street p7 `(“Compl.”) 1 8, 152` (`¶¶`), p8 `Id. 8.` (`¶` dropped; Azure `Id. 18.`), Garland p11 `id. 40`, Ironworkers p2 `at 1 12-16`, `at 1)20-23`. The readers differ, so a word-for-word comparison would not confirm them. |
| **real error, relied on** | **1** | Kuzdas p7: `... membership interests in the form of nonCOMPLAINT`. The line ends `non-` at the page foot, and both readers join it to the footer `COMPLAINT`. |
| real error, on a page in `suspect_pages` | 9 (`apple` report: 8) | Spliced transcript lines. Uber-O'Connor pp. 6-8, 11-14 (`... and essentially that 1 They're not employed through Uber.`), removed-1 p8 (`Did it happen on more incarcerated?`). The skill says not to quote from these pages. |

The `~` counts come from the automatic sort, whose classes overlap at the margins. The real-error counts are exact.

The earlier build's worst case, depo-removed-1 p14, gives no spliced sentences now. That was a two-column transcript page that `ocr` did not flag, where 21 spliced sentences were confirmed.

## 5. Plain terms

- **Values:** of the values the workflow lets a tool rely on, about **2 in 1,000** were really wrong (1 of 499 with `all`; 1 of 163, or 6 in 1,000, with `flagged`). The raw grading gives 6 in 1,000.
  - The one error is `ocr` taking `$45` for "$45 million" across a line break. It is not a misreading, and a careful by-value reconcile could catch it.
  - This rests on few examples: 14 real wrong values on the graded pages, 13 of them misreadings. Azure repeated none, and 11 of the 13 come from two stamp patterns.
  - A 95% upper bound for 1 in 499 is about 9 in 1,000, on clean, mostly born-digital filings.
- **Sentences:** of the sentences Azure confirms, about **0.5 in 1,000** were really wrong (1 of 2,176). It would be 4.6 in 1,000 (10 of 2,176) if a tool ignored `suspect_pages`. The raw grading says 27 in 1,000; nearly all of that is bad truth.
- **Residual risks found:**
  - Line-break handling of values (`$45` / `million`).
  - Page-end hyphens joined to footers.
  - `¶` read as a digit. Both readers do this, differently, so it is caught only if the workflow compares 1-2 digit tokens too.
  - Spliced columns on transcript pages, which now stay on flagged pages.

## 6. Caveats

- The 20 filings are mostly born-digital with clean scans. Only 139 pages could be graded, holding 15 graded wrong values.
- The token match is looser than reconciling by value. It ignores punctuation and `§`, and allows 3 characters between characters, which confirmed `§ 5` by coincidence. The sentence match skips any 1-2 digit token, which hid the `¶` misreads.
- Sentence grading depends on reading order: headers, footnotes and columns are placed differently by the OCR, Azure and the embedded text.
- `ocr_text_layer` missed ctr-marital-settlement, so 15 pages (9 values) were graded against an old OCR.
- The blind Markdown still lists `embedded_text_fixes` (the scan step uses the PDF's text even when verify ignores it), so the blind runs may be slightly better than a true scan.
- The committed `eval/verify_bundle/` predates this build. Until it is replaced, someone re-running the plan elsewhere would get the older numbers.
