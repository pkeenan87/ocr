# Residual risk with Azure on the flagged pages (work machine)

**Question:** after `ocr --verify-json` and Azure Document Intelligence on the pages it flags, how many values and sentences that the workflow would let a tool rely on are still wrong?

`ocr` does not call Azure. The Mac side of this measurement is already done and committed:

- `eval/verify_bundle/<os>/<run>/` holds the Mac's reports on the 20 public filings of `eval/public_set.json`, from `eval/verify_eval.py` on 2026-10-09 (master after PR 14), per macOS version:
  - `blind/`: the embedded text hidden from verify (`OCR_VERIFY_IGNORE_EMBEDDED=1`), so the readers decide alone, as on a scan;
  - `truth/`: the same scan with the embedded text, which grades every value.
- The runs are `apple-all` (`--verify-fields all`, which the skill uses) and `apple` (`--verify-fields flagged`).
- Apple-only runs verify nothing without embedded text (two Apple readings are not independent), so in the blind runs every value either goes to Azure or to a person. The blind reports send Azure 222 of 230 pages in `all` mode and 128 in `flagged` mode.
- The graded pages hold few wrong values: the Mac found 7, including the Jane Street stamp (see `eval/verify_bundle/README.md`). The result will say how often Azure repeats them, on a small sample; say so in the results.
- `eval/azure_verify_eval.py` simulates the workflow in `skills/ocr/SKILL.md` on those reports and Azure's readings, and grades the result. It needs only Python and pypdfium2, so the work machine needs no Mac build.

The Azure side runs on the work machine. Open Claude Code in the clone from the 2026-10-09 Azure comparison (`docs/azure-eval.md`, `docs/azure-eval-results.md`), or a fresh clone, and paste the prompt below.

---

## Prompt

You are measuring the residual risk of the `ocr` verification workflow with Azure AI Document Intelligence as the second service. Read `docs/azure-verify-eval.md` (this file), `eval/azure_verify_eval.py`, `skills/ocr/SKILL.md` (section "Values and quotes that will be relied on"), and `docs/azure-eval.md` sections 2 and 4 (credentials, API details) before doing anything.

**Privacy rule, absolute:** the only documents you may send to Azure are the 20 public court filings listed in `eval/public_set.json`. Never send any other file from this machine. Never print, log or commit the Azure key or endpoint.

### 1. Update the repo

1. In the existing clone, run `git checkout master` and `git pull`. If there is no clone, follow `docs/azure-eval.md` section 1 (clone, venv, `PYTHONUTF8=1` on Windows).
2. Make the bundle for this machine: build `ocr` from master, run `eval/verify_eval.py` for `--verify-fields all` and `flagged`, and copy both runs with `eval/export_verify_bundle.py` into `eval/verify_bundle/<os>/apple-all` and `eval/verify_bundle/<os>/apple` (for example `macos27`). Only `macos26` is committed so far; reuse a committed bundle only if it matches this machine's macOS version and the current `ocr`. If this machine runs another macOS version, or `ocr` has changed since, make a new bundle instead (`eval/verify_eval.py` for both modes, then `eval/export_verify_bundle.py` into `eval/verify_bundle/<os>/`) and use it below.
3. The venv needs only `eval/requirements.txt`. The interpreter is `PY` below, as in `docs/azure-eval.md`.

### 2. Azure readings

The 2026-10-09 comparison already analysed all 230 pages of the public set. Reuse those responses if they are still there:

1. Look for `eval/out/public/azure-di/<id>.azure.json` for every document in `eval/public_set.json`, and the PDFs in `eval/out/public/pdfs/<id>.pdf`.
2. For each JSON, check that `status` is `succeeded` (when present), that the result's `apiVersion` is `2024-11-30` and `modelId` is `prebuilt-layout`, and that `analyzeResult.pages` covers pages 1 to `n_pages_used`. Report any document that fails.
3. **If all 20 pass, make no Azure call.** Go to step 3.

Only if responses are missing or fail the check:

1. Download the missing PDFs as `docs/azure-eval.md` section 3.2 says.
2. Count the pages to analyse (`pages_arg`, or every page when it is `null`) and show the user the count and the cost at the price in `docs/azure-eval.md` section 7 (USD 10 per 1,000 pages on S0; re-check it). **Wait for an explicit OK.**
3. Analyse exactly as `docs/azure-eval.md` section 4 says (`prebuilt-layout`, `api-version=2024-11-30`, `outputContentFormat=markdown`, `stringIndexType=unicodeCodePoint`, no `features`, the document's `pages` range), and save each raw response as `eval/out/public/azure-di/<id>.azure.json`. Keep the glue code in `eval/out/azure/tools/` (gitignored).

Azure reads every page here, so one set of responses serves both routings below: the script only uses the pages a routing sends.

### 3. Run

Run all four, each with `--json eval/out/azure-verify/<run>-<route>.json`:

```bash
PY -I eval/azure_verify_eval.py --bundle eval/verify_bundle/<os>/apple-all --route report
PY -I eval/azure_verify_eval.py --bundle eval/verify_bundle/<os>/apple-all --route all
PY -I eval/azure_verify_eval.py --bundle eval/verify_bundle/<os>/apple --route report
PY -I eval/azure_verify_eval.py --bundle eval/verify_bundle/<os>/apple --route all
```

- `--route report` sends Azure only the report's `azure_pages`, which is the workflow as designed. `--route all` sends every page, as a reference.
- If the script fails on the Azure JSON shape, fix the reader (`azure_pages()`) and say so in the results; don't change the grading.

### 4. Read every example

The headline is "relied on but wrong". The embedded text is not perfect ground truth, so a number alone means little. For **every** item listed under "verified but wrong", "confirmed but wrong" and "Azure repeats a wrong value", and for the first 25 under "sentence Azure confirms, not in embedded text":

1. Render the page with pypdfium2 at 200 DPI to `eval/out/azure-verify/pages/` and look at the image.
2. Classify it:
   - **real error:** the printed page says something else, and the workflow would have relied on a wrong value;
   - **bad truth:** the page shows the value as read, and the embedded text is wrong or differs (a stamp the text layer doesn't carry, a footnote marker glued to a number, a different reading order);
   - **matching artifact:** the script's token match confirmed a value by coincidence (the value's tokens appear elsewhere on the page), so the workflow, reconciling by value, would not have confirmed it.
3. Note which real errors come from stamps, handwriting or overprinting. The known case is the Jane Street filing's stamp `04/24/24` printed over `04/18/24`, which Apple reads as `04/28/24` in both of its readings.

Recount the headline with only real errors. Report both the raw and the recounted numbers.

### 5. Results

Write `docs/azure-verify-eval-results.md` with:

1. **Environment:** the date; whether the Azure responses were reused from 2026-10-09 or new (and the cost, if any); OS and Python version.
2. **One table**, with a row per run and route: pages sent to Azure (and the share), values relied on, relied on but wrong (raw and real errors), wrong values stopped, right values sent to a person or disputed, sentences confirmed and unconfirmed.
3. **Azure alone:** the share of right values Azure has, and how many wrong values it repeats. This is the correlated-error rate, the main residual risk.
4. **Every real error**, with the document, page, the value as read, what the page says, and why both readers missed it.
5. **Plain-terms answer:** of the values the workflow lets a tool rely on, how many in a thousand are wrong, and of the sentences it lets a tool quote. Say how much rests on few examples.
6. **Caveats:** the 20 filings are mostly born-digital with clean scans; the token match is looser than reconciling by value; sentence grading depends on reading order.

### 6. Commit and push

1. Copy the four `--json` outputs to `docs/azure-verify-eval-results/`. Check each is under about 1 MB.
2. Commit only those paths, on a branch:
   ```bash
   git checkout -b azure-verify-eval
   git add docs/azure-verify-eval-results.md docs/azure-verify-eval-results/
   git status --short
   git commit -m "Eval: residual risk of ocr --verify-json with Azure on the flagged pages"
   git push -u origin azure-verify-eval
   ```
   - `git status --short` must show nothing else staged: no PDFs, no page images, no raw Azure responses, no `.env`, no glue code.
   - Never `git add -A` or `git add .`. Never push to `master`. If the push is refused, tell the user rather than forcing it.
3. Tell the user the branch name, whether Azure was called, and the headline numbers.
