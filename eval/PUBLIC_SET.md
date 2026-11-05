# Public court-filing set (20 documents, 230 pages)

> **Removed for privacy.** Four filings were part of the set when these results were measured but are not in this repository, because they contain personal details about private people: `ctr-marital-settlement`, `depo-removed-1`, `mot-removed-2` and `scan-removed-3`. Their manifest entries, OCR text and quoted examples are gone, and the last three have new labels. Figures here still describe all 20 documents, and rerunning from this repository covers the remaining 16.

This is a small real-world evaluation set with no ground truth. It contains public federal
filings from the CourtListener RECAP archive. They are OCR'd with Apple Vision and scored with
the dev quality judge (`eval/judge.py`, Jev; see `eval/JUDGE.md`). This set is the judge's first
test on real scans. The manifest is `eval/public_set.json`. The PDFs and all outputs live under
`eval/out/public/`, which is gitignored.

## Selection and provenance

- **Source.** Every document comes from the CourtListener RECAP archive: federal district and
  bankruptcy court documents bought from PACER and made public by RECAP users.
  - Candidates were found anonymously with the v4 search API
    (`/api/rest/v4/search/?type=r&available_only=on&q=...`), using no token and no account.
  - Queries searched descriptions for complaints with `document_number:1`
    (`court=cand cacd casd`), "memorandum of points and authorities", "memorandum of law",
    "order granting … motion to dismiss", depositions or transcripts filed as exhibits,
    `"Min-U-Script"` (condensed transcripts), exhibit agreements containing `"IN WITNESS WHEREOF"`,
    and habeas petitions and complaints filed before 2005 (old scans).
- **Download.** PDFs were downloaded from `storage.courtlistener.com/<filepath_local>` with the
  User-Agent `ocr-eval/0.1 (personal research)`.
  - Downloads were spaced about 2 s apart, and searches about 8 s apart.
  - The anonymous search API returned **HTTP 429** (`Retry-After: 6`) once searches came closer
    together than about 1 s. The helper honours `Retry-After`.
  - All 25 downloads succeeded: the 20 documents chosen plus 5 spares that were not used.
- **Page budget.** Documents over 15 pages use pages 1–15. Four long ones were trimmed to pages
  1–10 to stay near the target of about 200 pages: irhythm, visionpharma, janestreet and knight.
  The set totals **230 pages**.
- **Text layer.** The text layer was checked with pypdfium2. A page counts when it has more than
  200 characters of text; the PACER header stamp alone does not count.
  - 7 documents are image-only.
  - 3 documents are partial.
  - 10 documents have a text layer. Three of those are scans with an OCR'd layer: lozano,
    marital and nissan.
  - `ocr` defaults to `--use-text-layer never`, so **every page judged here is Apple Vision
    output**. `text_layer_pages` is empty in every output.

| id | category | court | date | pages used | text layer | what it is |
|---|---|---|---|---|---|---|
| cmpl-sec-kuzdas | complaint | N.D. Cal. | 2026-07-23 | 1-15 of 18 | yes | SEC complaint, 28-line pleading paper |
| cmpl-irhythm | complaint | N.D. Cal. | 2026-06-23 | 1-10 of 51 | yes | securities class action, pleading paper |
| cmpl-ab-sanbernardino | complaint | C.D. Cal. | 2026-07-10 | 1-15 of 29 | yes | § 1983 complaint, pleading paper |
| cmpl-us-lozano-1985 | complaint | N.D. Cal. | 1985 (docketed 2018) | 1-13 | scanned + OCR layer | typewritten 1985 complaint, pleading paper |
| cmpl-rcr-deline | complaint | D. Wyo. | 2000-02-03 | 1-15 of 25 | none | typed notice of removal + complaint, Courier |
| mot-removed-2 | motion_brief | (withheld) | (withheld) | 1-15 of 25 | none | pro se opposition MPA, pleading paper, scanned print |
| mot-visionpharma-mol | motion_brief | S.D.N.Y. | 2013-06-25 | 1-10 of 33 | none | memorandum of law ISO motion to dismiss, footnotes |
| mot-janestreet-opp | motion_brief | S.D.N.Y. | 2024-04-24 | 1-10 of 30 | partial | opposition memo, footnotes, block quotes |
| mot-cimontubo-reply | motion_brief | S.D.N.Y. | 2021-04-20 | 1-7 | yes | reply memo of law, Garamond, footnotes |
| depo-removed-1 | deposition | (withheld) | (withheld) | 1-15 of 24 | yes | full deposition, Min-U-Script **4-up** |
| depo-uber-oconnor | deposition | N.D. Cal. | 2015-07-23 | 1-15 | partial | deposition excerpt exhibit, Min-U-Script **4-up** |
| depo-knight-avery | deposition | E.D. Wis. | 2024-08-01 | 1-10 of 129 | yes | full-page boxed transcript exhibit |
| depo-christoforo-lupo | deposition | D. Mass. | 2007-09-17 | 1-5 | none | deposition excerpt exhibit, scanned |
| ctr-holt-settlement | contract | N.D. Ala. | 2019-06-21 | 1-7 | partial | settlement agreement exhibit, clerk stamp |
| ctr-marital-settlement | contract | (withheld) | (withheld) | 1-15 of 21 | scanned + OCR layer | marital settlement agreement exhibit (phone scan) |
| ctr-nissan-dealer | contract | N.D. Ill. | 2025-01-23 | 1-15 of 45 | scanned + OCR layer | dealer agreement, landscape 2-column small print, forms, signatures |
| ord-garland-dunkin | order | N.D. Cal. | 2024-05-31 | 1-14 | yes | order granting MTD, line-numbered, rotated court sidebar |
| ord-ironworkers-bogart | order | N.D. Cal. | 2012-06-13 | 1-10 | yes | order granting MTDs, heavy footnotes, rotated sidebar |
| scan-dean-habeas | scanned | M.D. Pa. | 2001-04-19 | 1-10 | none | pro se habeas petition on AO form, handwritten answers |
| scan-removed-3 | scanned | (withheld) | (withheld) | 1-4 | none | pro se § 1983 form, handwritten block capitals, FILED stamp |

The manifest has the docket numbers, case names, CourtListener and PDF URLs, and notes.

## Apple results

The scan command was
`ocr scan --model apple --jobs 4 --overwrite [--pages …]`, run with the isolated state
`eval/out/public/state/`. Judge thresholds are the calibrated defaults: garbled 0.45,
interleaved 0.35, repetition 0.55, table_broken 0.4, and readability < 3.5.

**Run statistics**
- Total wall time was 111 s for 230 pages, about 2.1 pages/s with 4 jobs on a machine busy with
  other evals.
- All 20 documents exited 0.
- **`flagged_pages` was empty on every document.**
- There was **1 segfault (signal 11) in 21 invocations**, on cmpl-rcr-deline's first attempt.
  The retry succeeded. It matches the known Vision init race.

**Judge cost.** 230 requests cost $0.021 in total.

### Per category

| category | pages | garbled | interleaved | repetition | table_broken (pages asked) | mean readability | readability < 3.5 |
|---|---|---|---|---|---|---|---|
| complaint | 68 | 56% | 15% | 0% | 33% (6) | 3.65 | 40% |
| motion_brief | 42 | 74% | 0% | 0% | – (0) | 3.77 | 26% |
| deposition | 45 | 84% | 31% | 0% | 94% (18) | 3.46 | 67% |
| contract | 37 | 81% | 14% | 0% | 100% (2) | 3.48 | 62% |
| order | 24 | 75% | 4% | 0% | – (0) | 3.78 | 25% |
| scanned | 14 | 100% | 36% | 0% | – (0) | 2.82 | 100% |
| **all** | **230** | **73%** | **15%** | **0%** | **81% (26)** | **3.57** | **48%** |

For reference, clean ground-truth legal text scored a mean of 4.69 in calibration, with 0% of
pages below 3.5 (JUDGE.md). Real filings are clearly harder.

### Per document

`col/tr` is the number of pages listed in `column_split_pages` / `transcript_pages`. The last
four columns count pages flagged by the judge.

| id | pages | wall s | exit | col / tr | mean read | read < 3.5 | garbled | interleaved | table_broken (asked) |
|---|---|---|---|---|---|---|---|---|---|
| cmpl-sec-kuzdas | 15 | 3.3 | 0 | 0 / 0 | 4.22 | 0 | 3 | 0 | 0 (4) |
| cmpl-irhythm | 10 | 2.2 | 0 | 1 / 0 | 4.05 | 0 | 5 | 0 | 0 (0) |
| cmpl-ab-sanbernardino | 15 | 6.9 | 0 | 1 / 0 | 4.20 | 2 | 3 | 1 | 2 (2) |
| cmpl-us-lozano-1985 | 13 | 12.2 | 0 | 4 / 0 | 2.72 | 12 | 13 | 8 | 0 (0) |
| cmpl-rcr-deline | 15 | 4.7 | 0 (1 segv, retried) | 2 / 0 | 3.10 | 13 | 14 | 1 | 0 (0) |
| mot-removed-2 | 15 | 9.7 | 0 | 2 / 0 | 3.54 | 6 | 13 | 0 | 0 (0) |
| mot-visionpharma-mol | 10 | 2.3 | 0 | 0 / 0 | 4.16 | 1 | 5 | 0 | 0 (0) |
| mot-janestreet-opp | 10 | 2.0 | 0 | 1 / 0 | 3.83 | 1 | 6 | 0 | 0 (0) |
| mot-cimontubo-reply | 7 | 1.4 | 0 | 0 / 0 | 3.64 | 3 | 7 | 0 | 0 (0) |
| depo-removed-1 | 15 | 14.7 | 0 | 13 / 13 | 3.51 | 9 | 11 | 6 | 9 (10) |
| depo-uber-oconnor | 15 | 10.3 | 0 | 11 / 11 | 3.53 | 11 | 12 | 3 | 6 (6) |
| depo-knight-avery | 10 | 3.4 | 0 | 6 / 0 | 3.45 | 5 | 10 | 3 | 1 (1) |
| depo-christoforo-lupo | 5 | 3.0 | 0 | 0 / 2 | 3.08 | 5 | 5 | 2 | 1 (1) |
| ctr-holt-settlement | 7 | 1.6 | 0 | 0 / 0 | 4.05 | 2 | 2 | 1 | 0 (0) |
| ctr-marital-settlement | 15 | 3.4 | 0 | 0 / 0 | 3.52 | 10 | 14 | 0 | 0 (0) |
| ctr-nissan-dealer | 15 | 10.0 | 0 | 9 / 0 | 3.17 | 11 | 14 | 4 | 2 (2) |
| ord-garland-dunkin | 14 | 6.9 | 0 | 13 / 0 | 4.05 | 0 | 8 | 0 | 0 (0) |
| ord-ironworkers-bogart | 10 | 5.4 | 0 | 10 / 0 | 3.40 | 6 | 10 | 1 | 0 (0) |
| scan-dean-habeas | 10 | 3.5 | 0 | 4 / 0 | 2.84 | 10 | 10 | 3 | 0 (0) |
| scan-removed-3 | 4 | 3.8 | 0 | 2 / 0 | 2.78 | 4 | 4 | 2 | 0 (0) |

Readability ranks the documents the way a human would:
- Born-digital pleadings score about 4.0–4.2.
- Contracts, depositions and scanned briefs score about 3.1–3.5.
- The 1985 typewriter complaint and the handwritten pro se forms score about 2.7–2.8.

## Spot-checks against the page image

The method:
1. Render each page with pypdfium2 and look at it.
2. Read the Apple Markdown and the judge's answers for the same page.

Fifteen pages were checked: 11 flagged and 4 unflagged. The images are in
`eval/out/public/png/`.

### Flagged pages (11)

| page | flags fired (value) | what is actually wrong | verdict |
|---|---|---|---|
| lozano p3 | garbled .93, interleaved .53, read 2.79 | A single-column typewriter page was **column-split down the middle**. Every line is cut in two and the halves are emitted separately ("the amount of $12,86" … "1.56 principal"). The signature reads "His! vsther A". | all correct |
| cimontubo p4 | garbled .94, read 3.35 | Born-digital Garamond, with r/t confusion: "memotandum", "debtot-creditor artangements", "entite", "partics'", "Treafise". "§ 31:1" became "S 31:1" and "words or terms" became "words of terms". | correct |
| removed-1 p10 | garbled .95, interleaved .65, table_broken .83, read 3.21 | A 4-up Min-U-Script page became a bogus 4-column Markdown table with quadrant lines merged ("3 In return when they -- 4 A No…"). Misreads include "Rigly", "vnthin", "Otner", "Identifled", "Idontrecall" and "wias". | all correct |
| knight p1 | garbled .91, interleaved .46, read 2.95 | The caption was column-split mid-word ("UNI … TED STATES", "KNI … GHT", "Plai … ntiff"). "LT. AVERY, OFFICER T.E." became "II. AVERY, OF OFFICER I.E.". | all correct |
| removed-3 p1 | garbled .98, interleaved .39, read 2.79 | Handwriting misreads: inmate "#885610" became "*883610", and "THE TEXAS DEPARTMENT OF CRIMINAL JUSTICE" became "DedTSRNS DAPAEMENT DE CAFMENAL". Form fragments are duplicated at the end of the page. | garbled and readability correct. interleaved is half right: the page has duplication and separated label/answer pairs, not true interleaving. |
| ironworkers p6 | garbled .98, interleaved .38, read 2.88 | Born-digital page. The footnote line beside the rotated "United States District Court" sidebar came out as gibberish ("podeculariraty beting konh pamäind woved…"). Footnote numbers were lost ("?7", "2 See Aronson", "3°"), and there is sidebar junk ("NN......", "i= ≤"). | all correct |
| holt p4 | garbled .97, interleaved .40, read 2.95 | Born-digital page. Two lines of the clean original ("virtue of this liquidated damages provision. In the event Employer must take action, …") were **replaced by gibberish**: "Sut no tmiedio ingeamaetimi com cilomce ay a lid". "employces" and "disclosethe" are also wrong. | garbled and readability correct. **interleaved is a false alarm**: nothing is interleaved. |
| nissan p7 | garbled .98, interleaved .35, table_broken .94, read 2.58 | The land and building tables were flattened into a bare column of numbers, and the small print is garbled ("I2seler's luthknent orils respors bribes"). The judge also **missed** a fluent error: Guide showroom "8,000" was read as "3,000". | correct, with one silent numeric error |
| ab-sanbernardino p12 | table_broken .84, interleaved .45 (read 3.57, not flagged) | Pleading prose was wrapped in a bogus 2-column table, margin numbers in one column and text in the other. The same paragraphs are then **repeated** below the table, with para 65 truncated. | table_broken correct. interleaved caught a real defect under the wrong name (duplication). **readability missed it.** |
| rcr-deline p5 | garbled .71, read 3.36 (interleaved .19) | Courier typewriter scan. "§1441(b)" was read as "$1441(b)" (by Vision itself, not by the currency fix) and "§1367" as "S1367". Other errors: "NHEREFORE", a dropped "4.", and phrase order shuffled in the last paragraph ("in the / the civil action now pending / State of Wyoming"). | garbled and readability correct. interleaved **missed** the reordering. |
| kuzdas p5 | garbled .58 only (read 3.89) | One real misread, "SZA Investments 9" for "S2A". Pleading margin numbers are scattered between the paragraphs. | borderline: one real misread is technically a "visible misread", but this is a good page. Readability is right not to flag it. |

### Unflagged pages (4)

| page | values | what is actually there | verdict |
|---|---|---|---|
| marital p9 | garbled .14, read 4.35 | Text is accurate. The only defect is the first line, which is duplicated as a `#` heading. | correct (clean enough) |
| removed-2 p14 | garbled .40, read 3.86 | Mostly accurate. It has "porjury", "PLAINTIFE'S" and "Ul claim" (for UI). Margin numbers are inline. | **near miss**: real misreads just under the 0.45 threshold |
| garland p2 | garbled .44, read 4.26 | **Every ¶ was misread**: "Id. 9 15-17", "Id. T18", "Compl. 9 60". There is junk from the rotated sidebar ("~n.n......", "SI"), and "I." / "LEGAL STANDARDS" are swapped. | **near miss**: the ¶→9 errors look like numbers to a text-only judge |
| removed-1 p13 | garbled .37, interleaved .23, read 3.68 | Each quadrant's text is correct, but the quadrants come out in the order **45, 47, 46, 48**. Line numbers are split onto their own lines, and "I" is read as "\|" several times ("before \| could"). | **miss**: wrong reading order is invisible to a plausibility judge |

### Judge accuracy on real filings

- **Page level.** Each of the 11 flagged pages really is defective.
  - 10 of them are clearly bad pages.
  - kuzdas p5 has one real misread on an otherwise good page.
  - So the judge raised no false alarm that a reviewer would act on.
  - Of the 4 unflagged pages, 1 is clean. Two have real misreads just under the garbled
    threshold, and 1 has a reading-order defect the judge cannot see.
- **Per flag.** 30 individual flags fired on the 11 flagged pages:
  - 26 point at the right defect.
  - 3 are debatable. Two interleaved flags fired on duplication, and kuzdas garbled fired on a
    single typo.
  - 1 is a false alarm: holt p4 interleaved.
- **Per question**
  - **garbled.** Right whenever it fires (10/10 on flagged pages, one borderline), but it fires
    on 73% of all pages. Every flag checked marks a real misread, though, so the share tracks how
    error-prone Apple is on real filings, not judge noise. As on the synthetic set, it means "at
    least one misread", and the threshold sits on a cliff: 0.40 and 0.44 both hid real misreads.
    Use it to compare engines, not as a per-page gate.
  - **readability < 3.5.** The most useful single signal: 9/9 correct on the flagged pages
    checked. It missed one structurally broken page (ab-sanbernardino p12, 3.57).
  - **table_broken.** 3/3 correct. On real filings it mostly catches *prose wrongly emitted as
    tables*: pleading pages and 4-up transcripts.
  - **interleaved.** The weakest question. It is correct on true column-split damage (lozano,
    knight, removed-1). But it also fires on duplication and gibberish (holt, ab-sanbernardino), and
    it misses phrase reordering on typed pages (rcr-deline p5).
  - **repetition.** Never fired. No loops were seen, which is consistent with Apple.
- **Blind spots.** These are confirmed on real pages:
  - wrong-but-plausible numbers (Nissan 8,000 → 3,000);
  - symbol substitutions that look like digits (¶ → 9);
  - wrong quadrant order in condensed transcripts;
  - dropped text. The judge cannot see omissions.

## What Apple gets wrong on real filings

1. **The column-split heuristic misfires on single-column pages.** `column_split_pages` is set on
   14 of the 20 documents. On the 1985 typewriter complaint (lozano p3) and the Knight caption
   (knight p1), it cuts lines in the middle, sometimes mid-word, and emits the halves as separate
   streams. Lozano is the worst document in the set: mean readability 2.72, with 12 of 13 pages
   below 3.5.
2. **Transcripts.**
   - The 4-up Min-U-Script pages are detected (`transcript_pages`), but they still come out as
     bogus Markdown tables with merged quadrants (removed-1 p10), or in the wrong quadrant order
     (removed-1 p13).
   - The full-page boxed Knight transcript is **not detected as a transcript at all**. The column
     split peels the line numbers into one block, drops every "Q", and leaves the "A" markers
     detached, so speaker attribution is lost (knight p6).
   - **`flagged_pages` is empty on all 20 documents**, although 67% of deposition pages have
     readability below 3.5.
3. **Pleading paper becomes a table.** On ab-sanbernardino p12, the line-number column and the
   prose became a 2-column table. The paragraphs were then emitted a second time below it.
4. **Legal symbols**, measured against the born-digital text layers:
   - **¶ is never output.** 0 of 48 survive; they become "9", "T" and the like ("Id. 9 15-17").
   - § survives 70 of 82 times. It is lost to "S" or "$" mostly on Courier typewriter scans
     ("$1441(b)", "S1367").
   - The `$` currency fix was right in the one document where it fired (Holt, 3 real amounts).
5. **Font-specific letter confusions on clean born-digital text.** The worst case is Garamond
   (cimontubo), where r is read as t: "memotandum", "artangements", "entite". A born-digital brief
   at 200 dpi still gets misread words on most pages.
6. **Gibberish lines on clean pages.** Two lines of a born-digital settlement agreement became
   nonsense (holt p4). A footnote line beside the rotated court sidebar was also garbled
   (ironworkers p6).
7. **Rotated sidebars.** "United States District Court / Northern District of California" leaks
   in as junk, such as "NN......", "i= ≤" and "~n.n......".
8. **Handwriting and forms.** Handwritten names and numbers are misread, with the inmate number
   wrong (removed-3 p1). Form labels and answers become separated. The pro se forms average
   readability about 2.8.
9. **Small print and 2-column landscape (Nissan).** Tables are flattened into columns of numbers,
   with silent digit errors.

## Stage B: model engines

This stage has not been run. It waits for the running evals to finish.

```bash
# Once per engine: install it into the ISOLATED public state (weights come from the HF cache).
eval/public_run_models.sh paddleocr-vl --setup
# Later runs: start the server on port 8091, scan the 20 documents, stop the server.
eval/public_run_models.sh paddleocr-vl
eval/public_run_models.sh olmocr --setup
eval/public_run_models.sh dots-ocr --setup

# Judge every engine that has outputs, side by side (Apple responses come from the cache).
spike/.venv/bin/python -I eval/public_compare.py            # or --engines apple,paddleocr-vl --ids depo-removed-1
```

`public_run_models.sh`:
- reads the document ids and page ranges from `eval/public_set.json`;
- uses `eval/out/public/state/{config,state}`, never `~/.config/ocr`;
- serves on port 8091 by default, so it cannot collide with the other evals on 8080, and refuses
  to start if the port is busy;
- stops the server on exit, including Ctrl-C;
- writes `eval/out/public/<engine>/runs.tsv` with the exit code and wall time per document;
- stops on exit 3 (server lost), so it can be re-run to resume.

`public_compare.py`:
- runs `judge.py --engine-compare` per document over the engines present, and saves the JSON to
  `eval/out/public/judge/compare/<id>.json`;
- prints an engine × category table of flag rates, mean readability and the share of pages below
  3.5;
- counts only pages that every engine produced.

Because this judge has the blind spots described above, also re-check a few pages per engine by
eye, especially for the failures a text-only judge cannot see: numbers, ¶ and quadrant order.

## Files

| path | status | content |
|---|---|---|
| `eval/public_set.json` | committed | manifest |
| `eval/public_run_models.sh` | committed | stage B scan driver |
| `eval/public_compare.py` | committed | stage B judge + aggregate |
| `eval/out/public/pdfs/` | ignored | PDFs |
| `eval/out/public/apple/` | ignored | Apple outputs |
| `eval/out/public/judge/apple/` | ignored | judge JSON and text per document |
| `eval/out/public/meta/` | ignored | search results, text-layer check, `apple_runs.json` (per-attempt exit codes and wall times) |
| `eval/out/public/png/` | ignored | spot-check renders |
