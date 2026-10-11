# Human-checked test set: plan for Patrick (60 pages)

**Why:** two recent changes looked safe on our measurements and were not.
- dots at 85 DPI was clean on synthetic pages, but dropped a paragraph on a real complaint.
- Plain-text dots dropped a paragraph and renumbered another on the 20th public filing, after five filings looked fine.

Our references are synthetic, which is too clean, or the PDF's own text, which is missing on scans and sometimes another OCR's layer. This set says, for each value on each page, what the page shows, checked by a person. Every engine and setting is measured against it, and nothing is ever tuned on it.

## The set

**18 public documents, 61 pages**, listed in `eval/holdout/manifest.json`. They are CourtListener RECAP filings, none of them in `eval/public_set.json`.

| Id | Pages | What |
|---|---|---|
| h01 | 2 | Old typed scan (1991), forfeiture decree |
| h02 | 4 | Old typed scan (1990), opinion |
| h03 | 4 | Typed or handwritten habeas petition form (2001) |
| h04 | 1 | Civil cover sheet (form) |
| h05 | 4 | Registration of a foreign judgment (form, amounts) |
| h06 | 2 | Returned summons (form) |
| h07 | 2 | Affidavit of service (form) |
| h08 | 5 | Settlement agreement (payment amounts) |
| h09 | 2 | Stipulated judgment on default in payments |
| h10 | 5 | Attorney fee motion (amounts) |
| h11 | 5 | Fee application with a financial table |
| h12 | 4 | Min-U-Script deposition transcript |
| h13 | 4 | Typed reporter's transcript (1990) |
| h14 | 4 | Motion to dismiss, footnotes, pleading paper |
| h15 | 3 | Pro se petition, likely stamped |
| h16 | 3 | Final judgment, amounts and interest |
| h17 | 2 | Order on prejudgment interest |
| h18 | 5 | Born-digital opinion (the control) |

Twelve of the pages (marked **(T)** in the review page) also get a full transcription.

## Tonight (Claude)

1. Download the 18 PDFs: `eval/holdout/prepare.py download`, into `eval/out/holdout/pdfs/`, which is gitignored. **This waits for your OK.**
2. Scan every document with Apple, Paddle and dots (`prepare.py scan`).
3. Build the review page (`prepare.py build`): `eval/out/holdout/review.html`, with the page images beside the values each reading found.

## Tomorrow, part 1: the review (you, about 5–8 hours, in as many sittings as you like)

1. Open `eval/out/holdout/review.html` in Chrome or Safari by double-clicking it. It works offline and sends nothing anywhere.
2. Type your name in **Reviewer name**.
3. For each page:
   - **Values:** for each value listed, make the text box match exactly what is printed, including `$`, commas, `§`, and the hyphens in a docket. Then press **Correct**.
     - Press **Not on page** if the reading invented or garbled it beyond recognition.
     - Tick **stamp**, **handwriting** or **form field** if that is where the value sits.
     - **Green** means every reading agreed, **yellow** that they differ. Check the green ones too: agreement is how two readers repeat the same mistake.
   - **Missed values:** use **+ Add a value the readings missed** for any amount, date, docket, Bates number, citation, percent or numbered paragraph on the page that is not listed.
   - **Page facts:** tick what applies (stamp, handwriting, form fields, checkboxes, redaction box, multi-column, transcript, table, footnotes, signature, rotated, blank).
   - **(T) pages:** correct the pre-filled transcription to match the page exactly, in reading order.
   - Tick **This page is fully reviewed**. It refuses while a value has no decision.
4. Your work is kept in the browser as you go. Press **Export JSON** at the end of each sitting, and save the file as `eval/holdout/holdout_v1.json` in the repo, replacing the last export. **Import** restores from an export, for example on another browser.
5. Say "review done" when all 61 pages show ✓.

**Rough timing:** values take 3–5 minutes a page, about 3–5 hours in all, and the 12 transcriptions take 10–15 minutes each, about 2–3 hours. Hard pages (h02, h03, h13) take longest.

**When in doubt:**
- A value you cannot read with certainty: mark it **Correct** with your best reading and say so in **Notes**.
- Something the readings show that is not really on the page: mark it **Not on page**.

## Tomorrow, part 2: the Azure re-measurement (work machine, about 30 minutes of your time)

The Azure residual-risk numbers (`docs/azure-verify-eval-results.md`) were measured before PRs 18–33. To refresh them on the work Mac, with no Azure calls (the cached responses are reused), paste this to Claude Code there:

> In the ocr repo: `git pull`, `cargo build --release`. Run `eval/verify_eval.py --engine apple --verify-fields all` and again with `--verify-fields flagged`. Copy both runs with `eval/export_verify_bundle.py` into `eval/verify_bundle/macos27/apple-all` and `eval/verify_bundle/macos27/apple`. Then follow `docs/azure-verify-eval.md` from step 3: reuse the cached Azure responses in `eval/out/public/azure-di/` and make no Azure call. Update `docs/azure-verify-eval-results.md` with the new numbers next to the old ones, and push to a branch `azure-verify-eval-2`.

## After the review (Claude)

1. Commit `eval/holdout/holdout_v1.json`. It holds only values, page facts, transcriptions and your decisions, with no images.
2. Write `eval/holdout/score.py`, which measures any engine and setting against it:
   - false-verified rate (values `verified` or confirmed that are wrong);
   - wrong values caught;
   - values dropped;
   - page facts detected;
   - strict and normalized CER and word error on the 12 transcribed pages;
   - each with a confidence interval and the worst documents.
3. Run it for Apple, Paddle and dots as they stand, and write up the baseline in `docs/spike.md`.
4. From then on, a speed or accuracy change ships only after it holds up on this set.

## Rules for the set

- **Never tuned on.** If a change fails here, the failing case is copied into the development sets (synthetic or public) and worked on there.
- **Frozen.** `holdout_v1.json` changes only to fix a reviewer mistake, recorded in its history. New documents make a `v2`.
- **Public documents only,** so they can go to Azure and the judge.
