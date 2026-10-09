# Azure Document Intelligence comparison (work machine)

**Purpose:** score Azure AI Document Intelligence `prebuilt-layout` (Markdown output) on the same synthetic fixtures and with the same scorer as the Mac engines in [docs/eval.md](eval.md), and judge it on the 20 public court filings of [eval/PUBLIC_SET.md](../eval/PUBLIC_SET.md).

**This is a reference only.** `ocr` does not call Azure, and nothing here changes the CLI. There is no runner script in the repo: Claude on the work machine calls the REST API directly and writes its own glue code outside the repo (or in `eval/out/`, which is gitignored). The output format contract below is what makes its results comparable.

The API details and the price were checked on 2026-10-08 against the Microsoft pages cited in the prompt. Re-check them on the day.

Open Claude Code in an empty working folder on the work machine (Windows, macOS or Linux) and paste the prompt below.

---

> **Transcript line numbers: compare raw against raw.** Since 2026-10-08 the Mac's `ocr` re-anchors transcript line numbers to page geometry, using Apple Vision line positions (`src/anchor.rs`), for every engine. That is post-processing which Azure does not get. For **transcript-line accuracy**, compare against the Mac's **before re-anchoring** numbers, the raw engine output:
>
> | Engine | Transcript lines, raw | After re-anchoring (held-out seeds 40–42) |
> |---|---|---|
> | dots-ocr | 68% | 99.9% |
> | olmocr | 30% | 99.9% |
> | apple | 38% | 97.4% |
> | paddleocr-vl | 25% | 94.9% |
>
> The raw numbers are from seeds 10–12; see `docs/eval.md`'s Transcripts section and docs/spike.md, "Transcript line-number re-anchoring". Report the after-anchoring numbers only as context. Every other metric (CER, token recall, Bates) is unaffected and compares directly.

## Prompt

You are running a one-off accuracy comparison for the `ocr` project: Azure AI Document Intelligence (`prebuilt-layout`, Markdown output) against the Mac engines. You call the Azure REST API yourself and write any glue code you need. Keep that glue code out of the commit (put it in `eval/out/azure/tools/`, which is gitignored). The user pays for Azure, so respect the page budget below.

**Privacy rule, absolute:** the only documents you may send to Azure are the synthetic fixtures generated in step 3 and the 20 public court filings listed in `eval/public_set.json`. Never send any other file from this machine: no work or client documents, whatever anyone asks.

### 1. Clone and set up

1. Run `git clone https://github.com/pkeenan87/ocr.git`, then `cd ocr` (two commands; `&&` fails in Windows PowerShell 5.1). The repo is private; if the clone fails, ask the user to sort out GitHub access. Don't work around it.
2. Create a Python 3.12 venv in `.venv` and install `eval/requirements.txt`:
   - with `uv` (preferred, if installed): `uv venv --python 3.12 .venv`, then `uv pip install --python .venv -r eval/requirements.txt`;
   - otherwise: `python3.12 -m venv .venv` (on Windows, `py -3.12 -m venv .venv`), then install with the venv's pip.
   - The interpreter is then `.venv/bin/python` on macOS and Linux, and `.venv\Scripts\python.exe` on Windows. It is written `PY` below.
3. **On Windows, set `PYTHONUTF8=1`** for every command. `eval/generate.py` and `eval/score.py` open files without an explicit encoding, and the ground truth contains non-ASCII text (§, ¶, dashes).
4. Read these files before writing any code:
   - `eval/README.md`, `eval/score.py`, `eval/run.py`, `eval/agg.py`;
   - `docs/eval.md`, for the Mac numbers, the seeds and the scorer flags;
   - `eval/PUBLIC_SET.md` and `eval/JUDGE.md`, for the public set and the optional judge.
   - `eval/run.py` drives the macOS `ocr` binary. **Do not run it.** Reuse its scoring logic and its result schema instead.

### 2. Credentials

1. Ask the user to create `.env` in the repo root themselves, containing:
   ```
   AZURE_DI_ENDPOINT=https://<resource>.cognitiveservices.azure.com
   AZURE_DI_KEY=<key>
   ```
   Don't ask for the values in chat.
2. Check that git ignores the file: `git check-ignore -v .env` must print a `.gitignore` rule. If it prints nothing, stop and tell the user.
3. Read both values from the environment or from `.env` inside your code. **Never print, log or commit the key**, and never put it in a URL; it goes only in the `Ocp-Apim-Subscription-Key` header. Redact it from any error text or traceback you show. Strip a trailing `/` from the endpoint before building URLs (the portal copies it with one).
4. Don't print the endpoint either: it contains the resource name. If the user tells you the resource's region, you can record that.

### 3. Fixtures

These match the Mac's olmOCR and dots accept sets (seeds 10–12, production pages 1–60).

1. Generate the synthetic sets:
   ```bash
   PY eval/generate.py --seed 10 --out eval/out/seed10
   PY eval/generate.py --seed 11 --out eval/out/seed11 --families pleading,contract,transcript4up,fax,table
   PY eval/generate.py --seed 12 --out eval/out/seed12 --families pleading,contract,transcript4up,fax,table
   ```
   - Seed 10 includes `production`, a 500-page file. **Send only pages 1–60 of it** (`pages=1-60`) and score it against the first 60 ground-truth pages.
   - From the other families, send every page.
   - Record the installed `reportlab`, `pypdfium2` and `Pillow` versions. The ground truth is text and is identical across versions; the rendered PDFs may differ slightly if these differ from `eval/requirements.txt`.
2. Download the 20 public PDFs listed in `eval/public_set.json` (key `documents`, field `pdf_url`) to `eval/out/public/pdfs/<id>.pdf`:
   - Use the User-Agent `ocr-eval/0.1 (personal research)` and wait about 1 s between downloads.
   - Honour `Retry-After` on HTTP 429.
   - Check that every file starts with `%PDF-`.
   - Each document's page range is `pages_arg`, for example `"1-15"`. If it is `null`, send the whole document (`n_pages_used` pages). Every range starts at page 1.

### 4. API details (verified 2026-10-08; re-verify)

Sources:
- Analyze Document: https://learn.microsoft.com/en-us/rest/api/aiservices/document-models/analyze-document?view=rest-aiservices-v4.0%20(2024-11-30)
- Get Analyze Result: https://learn.microsoft.com/en-us/rest/api/aiservices/document-models/get-analyze-result?view=rest-aiservices-v4.0%20(2024-11-30)
- Layout model: https://learn.microsoft.com/en-us/azure/ai-services/document-intelligence/prebuilt/layout?view=doc-intel-4.0.0
- Markdown elements: https://learn.microsoft.com/en-us/azure/ai-services/document-intelligence/concept/markdown-elements?view=doc-intel-4.0.0

**Before you start, re-open these pages.** If a newer GA api-version exists, tell the user, and use 2024-11-30 unless they decide otherwise.

**Version.** v4.0, `api-version=2024-11-30`, which is GA. The layout page says to use it for all new development.

**Submit:**
```
POST {endpoint}/documentintelligence/documentModels/prebuilt-layout:analyze?_overload=analyzeDocument&api-version=2024-11-30&outputContentFormat=markdown&stringIndexType=unicodeCodePoint[&pages=1-15]
Ocp-Apim-Subscription-Key: <key>
Content-Type: application/json

{"base64Source": "<base64 of the PDF>"}
```
- `_overload=analyzeDocument` appears in Microsoft's documented URL. If the service rejects it, drop it and note that.
- `pages` takes 1-based numbers or ranges, for example `1-3,5,7-9`.
- `outputContentFormat` is `text` or `markdown`; use `markdown`.
- `stringIndexType=unicodeCodePoint` is the value the reference says to use for Python, so span offsets index Python `str` directly. The default, `textElements`, counts grapheme clusters, and its offsets can drift.
- **Don't** pass `features` (for example `ocrHighResolution`). Those are billed add-ons, and the Mac engines had no equivalent.
- A successful submit returns **202** with an `Operation-Location` header (the result URL) and `Retry-After` (an integer number of seconds).

**Poll:**
- `GET` the `Operation-Location` URL with the same key header, waiting `Retry-After` seconds between polls (default 2 s).
- `status` is one of `notStarted`, `running`, `succeeded`, `failed`, `canceled` or `skipped`.
- On `succeeded`, use `analyzeResult`. On `failed`, record `error` for that document and go on with the next one.

**Errors:**
- On 429 and 5xx, back off exponentially (honour `Retry-After` when present) and retry at most 5 times.
- Don't retry 4xx errors other than 429; report them, with the key redacted.

**Limits:** on S0, PDFs up to 2,000 pages and 500 MB. **On the free F0 tier, only the first two pages of each document are processed**, so if the resource is F0, stop and tell the user: the run would be meaningless.

**Response fields you need:**
- `analyzeResult.content`: the whole document as one Markdown string.
- `analyzeResult.pages[]`, each with:
  - `pageNumber`: 1-based, in the input document;
  - `spans[]`: `{offset, length}`, the location of the page in `content`;
  - `height` (in inches for PDFs);
  - `lines[]`: each with `content` and `polygon`, a list of x, y pairs in inches from the top-left corner.
- `analyzeResult.paragraphs[]`, each with:
  - `content`;
  - `role`: `pageHeader`, `pageFooter`, `pageNumber`, `title`, `sectionHeading`, `footnote` or `formulaBlock`;
  - `boundingRegions[]`: each with `pageNumber` and `polygon`.

**Markdown conventions:**
- Tables are HTML: `<table>`, `<tr>`, `<th>`, `<td>`, with optional `<caption>`, `rowspan` and `colspan`.
- Figures are `<figure>` with `<figcaption>`.
- Selection marks are ☒ and ☐.
- Page metadata is wrapped in HTML comments: `<!-- PageHeader="..." -->`, `<!-- PageFooter="..." -->`, `<!-- PageNumber="..." -->`.
- Page boundaries are marked with `<!-- PageBreak -->`.

**Splitting into pages:**
- Use `pages[].spans`, **not** the PageBreak comments. Page *n*'s Markdown is the concatenation of `content[offset:offset+length]` over its spans, in order.
- Assert that the spans don't overlap, and report any content between them that belongs to no page.
- Use PageBreak only as a cross-check: the number of breaks should equal the number of pages minus 1.

Save each raw response as `<name>.azure.json` next to its `.md`, for auditing. Results are cached on disk, so a re-run must skip documents that already have a `succeeded` JSON.

### 5. Output format contract (exact)

`eval/score.py` splits an output file into pages on its own markers, using `MARKER = <!--\s*page\s+(\d+)([^>]*?)-->`. A page that no marker opens scores as empty, so this format decides whether the comparison is fair.

**Paths:**
- `eval/out/azure/seedN/<family>.md` for the synthetic sets;
- `eval/out/public/azure-di/<id>.md` for the public set. `eval/public_compare.py` looks for `eval/out/public/<engine>/<id>.md`.

**Encoding:** UTF-8 with `\n` line endings. On Windows, open files with `encoding="utf-8", newline="\n"`.

**Front matter (optional):** if present, it must be the very first bytes of the file:
```
---
source: <file name>
pages: <page count>
engine: azure-di prebuilt-layout (2024-11-30)
---
```

**Page markers:**
- Each page begins with `<!-- page N -->` on its own line, then a blank line, then the page body.
- `N` is Azure's `pageNumber`. Every range starts at 1, so it equals the ground-truth page index.
- Pages appear in ascending order, and every analysed page gets a marker, even an empty one.

**Bates:**
- When a Bates number is found, the marker becomes `<!-- page N · Bates XYZ -->`. The separator is a space, U+00B7 (the middle dot) and a space, exactly as in `src/markdown.rs`.
- The regex, copied from `src/markdown.rs`, is `\b[A-Z]{2,}[-_ ]?\d{5,}\b`.
- **Bottom-of-page rule** (Mac: `BATES_ZONE = 0.08`, "blocks starting in this bottom fraction of the page", first match wins):
  - Candidates are the page's `lines[]` whose top edge (the smallest y in `polygon`) is at least `0.92 × page height`, plus the page's paragraphs with role `pageFooter` or `pageNumber`.
  - Take them top to bottom, and use the first regex match.
  - Don't search anywhere else on the page.

**Strip** these from page bodies, after reading any Bates number from them:
- `<!-- PageHeader="..." -->`;
- `<!-- PageFooter="..." -->`;
- `<!-- PageNumber="..." -->`;
- `<!-- PageBreak -->`.

Use a pattern like `<!--\s*Page(Header|Footer|Number|Break)\b.*?-->`, with DOTALL.

**Keep everything else as is:**
- HTML tables (the scorer flattens `<table>` into `cell | cell` rows);
- headings;
- `<figure>` and `<figcaption>` (the scorer drops the tags and keeps the text);
- ☒ and ☐;
- the text of any header or footer that Azure left as an ordinary paragraph.

**Why stripping is fair:** the scorer deletes all HTML comments, and it removes the ground-truth furniture (running headers, footers and Bates strings) from the output before comparing. So keeping or dropping headers and footers is neutral, exactly as for the Mac engines (olmOCR drops them, Apple and Paddle keep them). Write this list of stripped and kept items into the results doc.

**Check before scoring everything.** After the first synthetic file is written:
```python
import sys; sys.path.insert(0, "eval")
import json, score
md = open("eval/out/azure/seed10/pleading.md", encoding="utf-8").read()
pages = score.split_pages(md)
gt = json.load(open("eval/out/seed10/pleading.gt.json", encoding="utf-8"))
assert sorted(pages) == list(range(1, len(gt["pages"]) + 1)), sorted(pages)
print({n: extra for n, (body, extra) in pages.items()})          # Bates should show on pleading pages
print(score.score(gt, md, strip_line_numbers=True)["cer"])
```
Then fix any format problem before you run the rest.

### 6. Score

Use `eval/score.py` with the Mac flags from `docs/eval.md` and `eval/run.py`:
- `--strip-line-numbers` (`strip_line_numbers=True`) for the **pleading** family only, and for no other family;
- furniture ignored, so no `--keep-furniture` (`keep_furniture=False`) for every family;
- **production:** score against the first 60 ground-truth pages only (`gt["pages"] = gt["pages"][:60]`, as `run.py --first-pages 60` does);
- every other family against all its ground-truth pages.

The CLI is `PY eval/score.py <gt.json> <output.md> [--strip-line-numbers]`, but production needs the truncation, so it is simpler to import `score.score()` as above.

**Results file:** write `eval/out/azure/azure.json` in `run.py`'s schema, so that `eval/agg.py` works on it:
- `{"tag": "azure-di", "engine": "azure-di prebuilt-layout (2024-11-30)", "runs": {<family>: {"seed10": entry, ...}}}`;
- each `entry` holds `pages`, `wall_s`, `cer`, `dist_sum`, `len_sum`, `tokens_found`, `tokens_total`, `bates_ok`, `bates_total`, `tl_ok`, `tl_total`, `pages_in_output` and `cer_pages`;
- `wall_s` is the submit-to-succeeded time per document.

**Per-family table:**
- Pool the seeds the way `run.py`'s `aggregate()` does: CER is the sum of `dist_sum` over the sum of `len_sum`; recall, Bates and transcript lines are pooled the same way.
- Use the column layout of the `docs/eval.md` tables: `| Family | Pages | CER | Token recall | Bates | Transcript lines |`.
- Then run `PY eval/agg.py eval/out/azure/azure.json` for the aggregate without transcripts and production, which is how the Mac aggregates are shown.

**Judge (public set), optional:**
- Run it **only if** `AI_GATEWAY_API_KEY` is in the environment or `.env`. Otherwise skip it and say so in the results.
- If it is available, run `PY -I eval/public_compare.py --engines azure-di`. It reads `eval/out/public/azure-di/<id>.md`.
- Compare against the Apple table in `eval/PUBLIC_SET.md`; the thresholds are the same calibrated defaults.
- Judging 230 pages cost about $0.02 for Apple.
- The judge sends page text to the Vercel AI Gateway. That is allowed for public documents only (`eval/JUDGE.md`).

- **Transcript lines:** compare against the Mac's *raw* (before re-anchoring) numbers, not the re-anchored ones. See the "compare raw against raw" note at the top of `docs/azure-eval.md` for the table and the reason.

### 7. Cost, then the user's OK

Count the pages first, without calling Azure:

| Set | Pages |
|---|---|
| synthetic: seeds 10–12, five families | about 46 (11 pleading, 14 contract, 9 transcript4up, 6 fax, 6 table on the Mac; recount from the `.gt.json` files) |
| production, seed 10, pages 1–60 | 60 |
| public set | 230 (sum of `n_pages_used`) |
| **total** | **about 336** |

**Price:**
- On 2026-10-08, the Azure Retail Prices API (`https://prices.azure.com/api/retail/prices`, product "Azure Document Intelligence", SKU S0, eastus) listed meter **"S0 Pre-built Pages" at USD 10.00 per 1,000 pages**.
- The pricing page (https://azure.microsoft.com/en-us/pricing/details/ai-document-intelligence/) files Layout under "All Prebuilt Models", but renders the amounts client-side, so check it in a browser.
- The add-on meter (`S0 Add-on for Pages`, USD 6 per 1,000) applies only if you enable `features`; don't.
- **Re-check the price for the resource's region and tier**, then show the user the page count and the estimate (about USD 3.40 at that price) and **wait for an explicit OK** before the first real call.
- Do a single-document smoke test first (seed 10 `table`, 2 pages), check its output against section 5, then run the rest.
- A re-run must reuse cached results, so failures don't pay twice.

### 8. Results

Write `docs/azure-eval-results.md` with:

1. **Environment:**
   - the date;
   - the api-version and model;
   - the region, only if the user gave it (never the endpoint);
   - the pricing tier, OS and Python version, and the `reportlab`/`pypdfium2` versions;
   - the pages analysed and the actual cost estimate.
2. **Output handling:** what was stripped and what was kept (section 5), and the Bates rule.
3. **Synthetic results:** one table per family, with Azure next to apple, paddleocr-vl, dots-ocr and olmocr from `docs/eval.md`.
   - **Label the seeds.** Azure and dots/olmOCR used seeds 10–12; Paddle used 10–19; Apple used 10–39 at 200 DPI.
   - **Production:** Azure and dots/olmOCR are on pages 1–60; Apple and Paddle on all 500 pages.
   - Note that 3 seeds is 6–14 pages per family, so a single page moves the numbers by whole points.
4. **Public set:** the judge table, or "judge skipped: no AI_GATEWAY_API_KEY".
5. **Throughput:** wall time per document and pages per second, **labelled as cloud latency, not comparable** to the local Mac figures. It includes upload, queueing and polling.
6. **Caveats:**
   - the fixtures are synthetic;
   - fixture rendering versions;
   - the Bates rule is approximated from Azure's line geometry;
   - Azure Bates comes from Azure itself, whereas the Mac model engines take it from the Apple side-pass;
   - anything the format check or the span check turned up.

**Commit and push:**
1. Copy the small score summaries to `docs/azure-eval-results/`:
   - `azure.json` from step 6;
   - the public judge aggregate as JSON, if the judge ran.
   - Check each file is under about 1 MB.
2. Commit only those paths:
   ```bash
   git checkout -b azure-eval
   git add docs/azure-eval-results.md docs/azure-eval-results/
   git status --short    # nothing else staged: no PDFs, no raw outputs, no .env, no glue code
   git commit -m "Eval: Azure Document Intelligence comparison results"
   git push -u origin azure-eval
   ```
   - Never `git add -A` or `git add .`.
   - Never push to `master`.
   - If the push is refused, tell the user rather than forcing it.
3. Optionally, if Notion is connected, create a page titled **"ocr — Azure Document Intelligence comparison"** with the same content.
4. Nothing needs cleaning up in Azure: no resources were created.
5. Tell the user the branch name, the pages analysed, and the headline numbers.
