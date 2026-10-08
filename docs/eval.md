# Phase 6 evaluation (2026-10-08)

Harness: `eval/` (see [eval/README.md](../eval/README.md)). Everything here is reproducible from the seeds.

## Read this first

- **The fixtures are synthetic and template-based.** `eval/generate.py` builds pleadings, contracts, 4-up transcripts, fax scans, tables and a 500-page production from real-sentence templates with exact ground truth. They are not real filings. Real documents will be messier.
- **There is no handwriting family.** Trustworthy ground truth for handwriting cannot be generated (a handwriting font is not handwriting), so handwritten annotation pages are checked by hand and are not scored.
- Each seed is tiny (2-5 pages per family), so each number below pools many seeds (tune: 3 to 10, accept: 3 to 30). Percentages on 6-25 pages move by whole points on a single page. Treat the dots and olmOCR accept rows (3 seeds, 6-14 pages per family) as indicative.
- **Primary legal metrics are token recall and Bates accuracy.** CER is secondary.

## Binaries

| Used for | Binary built from | Notes |
|---|---|---|
| Apple tune and accept, Apple 500-page production, Paddle tune, dots tune | `570c3f8` (sources identical to `6b25df7`; no code change affected Phase 6 scoring) | the `results` files print `git 570c3f8` (tune) or `git c59c933` (HEAD at that moment; sources unchanged) |
| Paddle, dots and olmOCR accept; Paddle 500-page production; dots and olmOCR 60-page slices; legal-cleanup; pleading table | **`2631d47`**, "per-engine default DPI" | The results files print `git 2631d47` or `git 3659093`: that is only `HEAD` when the run started. The binary (`target/release/ocr`, built 09:45 on 2026-10-08) was **not** rebuilt after `2631d47`. |

- `3659093` merged the Apple Vision start-up crash fix (Vision capped at 2 concurrent requests, plus a supervisor that re-runs a crashed child). **No number below comes from a binary with the cap.** Apple accuracy is unaffected by the cap, which only limits concurrency. **Apple throughput under the cap is pending** (coordinator is re-measuring the 300-page benchmark): `TODO(apple-capped-pages/s)`.
- Every earlier eval number (from before Phase 3) was discarded.
- The user's real `~/.config/ocr` and `~/Library/Application Support/ocr` were never touched: all runs use `eval/out/state/{config,state}`. Weights came from the shared HF cache.

## Method

- **Run:** `eval/run.py` scans each fixture with `target/release/ocr scan --overwrite --quiet`, one engine and one server at a time, and scores the Markdown with `eval/score.py`. It reads `/usr/bin/time -l` "peak memory footprint" for the `ocr` process, and `vmmap --summary` "Physical footprint (peak)" of the server pid from `server.json`, once after the run (peak since server start; the server was restarted per phase).
- **Scoring, identical flags for every engine:**
  - both sides are normalized the same way (NFKC, quotes and dashes unified, Markdown and HTML table syntax removed, whitespace collapsed);
  - `--strip-line-numbers` for the `pleading` family only (strips a leading 1-2 digit margin number from output lines); no other family uses it;
  - ground-truth furniture (running headers, footers, Bates strings) is removed from the output before comparison, so keeping or dropping it is neutral (`--keep-furniture` scores it too; used only for the legal-cleanup check);
  - CER is micro-averaged (total edit distance over total reference characters, pooled over all seeds of a family); token recall is tokens found over tokens total (case numbers, amounts, dates, invoice numbers); Bates accuracy is the share of pages whose marker carries exactly the right Bates; transcript line accuracy is the share of numbered ground-truth lines that appear exactly as `N text`.
- **How the flags treat each engine differently.**
  - *olmOCR drops headers, footers and Bates numbers* from its text. The Bates number in the page marker comes from the Apple side-pass. Because furniture is stripped from the output before comparing, dropping it costs nothing in CER.
  - *Pleading margin numbers.* Apple drops them (bbox filter). Paddle keeps them inline, usually as a leading number that `--strip-line-numbers` removes, but when one is glued onto a paragraph number (`11.` read as `21.`), or lands in a table, the scorer cannot undo it. dots and olmOCR scored 0.0% CER on the seed 10 pleading both with and without `--strip-line-numbers`, so they emit no margin numbers. So Paddle's pleading CER is inflated by margin numbers and is not comparable to the others; its token recall is not.
  - *Transcripts.* Line numbers are part of the reference, so an engine that drops them (Apple, olmOCR) loses line accuracy and some CER, not necessarily token recall.
- **Apple aggregates exclude `transcript4up`**: transcripts are out of scope for Apple by design (it flags those pages), so that family is shown on its own row.
- Seeds: 0-9 are tune seeds, 10-39 accept seeds, never looked at while tuning. Production is seed 10.

## Split and what was reduced

| Engine | Tune | Accept | Production |
|---|---|---|---|
| apple | DPI 150, 200, 250 on seeds 0-9 | seeds 10-39 at 200 (default) and 150 (reported only) | 500 pages, seed 10 |
| paddleocr-vl | DPI 150, 200 on seeds 0-4 | seeds 10-19 | 500 pages, seed 10 |
| dots-ocr | DPI 100, 150, 200 on seeds 0-2 | seeds 10-12 | **`--pages 1-60` slice** of seed 10 |
| olmocr | none (fixed 1288 px) | seeds 10-12 | **`--pages 1-60` slice** of seed 10 |

Reduced: the full 500-page olmOCR and dots runs were skipped (about 2.2 h and 1.6 h on a quiet machine); the 60-page slices are scored against the first 60 ground-truth pages. Model-engine accept uses 10 or 3 seeds, not 30, because of time (olmOCR is about 16 s/page, dots 12 s/page at 100 DPI). Decision rules were written down before the tune results were read (`eval/out/decisions.md`, not committed).

## Tuning and decisions

Rule (written first): choose the lowest DPI whose token recall is within 0.5 pt of the best, whose Bates accuracy is not lower, and whose CER is within 0.5 pt of the best; ties go to the lower DPI. Primary metrics outrank CER.

### Apple (tune seeds 0-9, 121 pages, transcripts excluded)

| DPI | Token recall | CER | Bates |
|---|---|---|---|
| 150 | **98.2%** | 3.10% | 33/33 |
| 200 (default) | 97.4% | 3.29% | 33/33 |
| 250 | 97.35% | 3.22% | 33/33 |

By family (token recall at 150 / 200 / 250): pleading 97.8 / 97.4 / 99.2, contract 97.1 / 96.2 / 96.2, fax 96.8 / 91.0 / 93.0, table 98.8 / 98.6 / 97.2.

**An Apple DPI other than 200 scored better: 150.** It also did on the accept seeds (30 seeds, 4 families, 378 pages): token recall 98.4% vs 97.4%, CER 1.94% vs 2.32%, driven by the fax family (96.3% vs 88.5% recall). **The default stays 200**: the Apple goldens were generated at it and must remain byte-identical, and the user decided to keep it (PLAN §8). This is the user's call. Speed is the same at both; 150 uses a little less memory (`ocr` peak 210 MB vs 235 MB on pleading).

### Paddle (tune seeds 0-4, 72 pages, all families)

| DPI | Token recall | CER | Bates | Transcript lines |
|---|---|---|---|---|
| 150 | **98.65%** | 16.0% | 16/16 | 47.3% (seed-pooled) |
| 200 (old default) | 97.20% | 14.9% | 16/16 | 43.6% |

The recall gap is all in `transcript4up` (91.8% vs 83.0%); the other four families are 100% at both. Neither DPI satisfied the written rule (200 fails recall by 1.45 pt, 150 fails CER by 1.1 pt). The rule did not anticipate that, so I applied the stated priority (recall before CER) and chose **150**. Weak evidence: 12 transcript pages. Timings of this run were taken on a busy machine and are not used.

### dots (tune seeds 0-2, 44 pages, all families)

| DPI | Token recall | CER | Bates | Transcript lines |
|---|---|---|---|---|
| **100** | 99.84% | **0.54%** | 9/9 | 95.1% |
| 150 | 99.84% | 1.05% | 9/9 | 52.6% |
| 200 (old default) | 99.84% | 1.14% | 9/9 | 42.4% |

Recall and Bates are identical, CER and transcript lines are best at 100, and it is about 3x faster than 200 (9-12 s/page vs 35 s). Decision: **100**. Timings of the 150 and 200 runs were taken under contention and are not used.

### Code change (commit `2631d47`)

`src/cli.rs`: `effective_dpi()` now returns 150 for paddleocr-vl and 100 for dots-ocr; apple (and the Apple side-pass of olmOCR) keep 200. `tests/dots.rs` and `tests/paddle.rs` asserted the 1700x2200 image size; they now assert 850x1100 and 1275x1650 (the intent, "the default DPI", is unchanged). `cargo test --release` passed before the commit. The side-pass for Paddle and dots therefore also runs at 150 and 100 DPI; Bates accuracy stayed 100% on the tune and accept pleading seeds for both.

## Results (accept seeds, binary `2631d47` for models, `570c3f8` for Apple)

**Timing caveat.** Throughput figures in these tables are *not* quiet-machine figures: from 08:50 a crash-fix agent was building and running Vision loops, and from about 10:20 to 12:58 a second model server (dots, port 8091) shared the GPU, which cut combined throughput well below one server alone. Model-engine accept timings are also dominated by per-process start-up and model warm-up on 2-5 page files. Use the "Throughput and memory" section for speed; the tables below are for accuracy.

### apple (default 200 DPI, 30 seeds)

| Family | Pages | CER | Token recall | Bates | 
|---|---|---|---|---|
| pleading | 105 | 1.7% | 97.9% | 88.6% |
| contract | 153 | 2.3% | 96.9% | - |
| fax | 60 | 7.7% | 88.5% | - |
| table | 60 | 0.3% | 98.6% | - |
| **aggregate without transcripts** | 378 | 2.32% | 97.4% | 93/105 |
| production (500 pages, seed 10) | 500 | 1.1% | 96.1% | 99.4% |
| *transcript4up (out of scope, shown separately)* | 78 | 63.0% | 83.4% | line accuracy 38.3% |

At 150 DPI (reported only): pleading 2.1% / 98.2% / 87.6%, contract 1.5% / 97.5%, fax 7.1% / 96.3%, table 0.2% / 98.9%, transcript4up 63.0% / 87.6% / 45.0% lines.

### paddleocr-vl (150 DPI, seeds 10-19)

| Family | Pages | CER | Token recall | Bates | Transcript lines |
|---|---|---|---|---|---|
| pleading | 36 | 13.3% | 98.7% | 100% | - |
| contract | 48 | 0.1% | 100% | - | - |
| transcript4up | 25 | 51.1% | 81.5% | - | 25.2% |
| fax | 20 | 0.0% | 100% | - | - |
| table | 20 | 0.0% | 100% | - | - |
| production (500 pages, seed 10) | 500 | 0.0% | 100% | 99.4% | - |

### dots-ocr (100 DPI, seeds 10-12)

| Family | Pages | CER | Token recall | Bates | Transcript lines |
|---|---|---|---|---|---|
| pleading | 11 | 0.0% | 100% | 100% | - |
| contract | 14 | 0.2% | 100% | - | - |
| transcript4up | 9 | 1.2% | 100% | - | 68.1% |
| fax | 6 | 6.3% | 100% | - | - |
| table | 6 | 0.1% | 100% | - | - |
| production, pages 1-60 | 60 | 0.6% | 99.8% | 100% | - |

### olmocr (fixed 1288 px, seeds 10-12)

| Family | Pages | CER | Token recall | Bates | Transcript lines |
|---|---|---|---|---|---|
| pleading | 11 | 0.0% | 99.5% | 90.9% | - |
| contract | 14 | 0.0% | 100% | - | - |
| transcript4up | 9 | 11.2% | 96.9% | - | 30.4% |
| fax | 6 | 0.0% | 100% | - | - |
| table | 6 | 0.1% | 98.9% | - | - |
| production, pages 1-60 | 60 | 0.1% | 100% | 100% | - |

olmOCR's Bates comes from the Apple side-pass, so its pleading Bates (90.9%, 1 page in 11) is Apple's weakness there, as in Apple's own 88.6%. Table pages 1 of seed 10 and seed 12 fell back to Apple (exit 1, flagged); that is the likely cause of the 98.9% table recall.

### Transcripts

| Engine | Token recall | Lines exact |
|---|---|---|
| dots-ocr | 100% | 68.1% |
| olmocr | 96.9% | 30.4% (drops the line numbers) |
| apple | 83.4% | 38.3% (flags the pages) |
| paddleocr-vl | 81.5% | 25.2% |

**dots is the best engine for transcripts, but line numbers from every engine are unreliable.** Transcripts number every physical line, including wrapped continuations. Paddle reads the words correctly, but joins a continuation onto the line above and renumbers its output sequentially, so every number after the first wrap is off by one and the drift grows down the page; on a whole 4-up sheet it also invents text. (A probe on seed 10 sheet 1, 99 numbered lines: Paddle full page 16-20 lines exact at 150-300 DPI, quadrant crops 42, the remaining crop errors being almost entirely the renumbering.) The earlier "100/100" Paddle spike result came from a fixture with short identical lines that never wrap. **Verify page:line citations against the PDF.** A possible fix, left for the user to decide: re-anchor line numbers using the Apple side-pass's physical line geometry. I did not recommend Paddle for transcripts here for that reason.

I checked a failing page with `score.py --diff-page` (Paddle, seed 1, page 1, 200 DPI). That page fell back to Apple (listed in `fallback_pages` and `flagged_pages`), so the garbling there (`You SOLLO`, `cal1?`, `$8,029.00` read as `5o,U29.UU`) is Apple Vision on dense 4-up text, not Paddle. Paddle's own failures on transcripts are the renumbering and invented text described above, so the low recall is real, not a scorer artifact. Pooled Paddle transcript recall also depends on how many pages fall back.

## Throughput and memory

Quiet-machine figures where I have them; the source of each is stated.

| Engine | Pages/s | Source | 300-page estimate | `ocr` peak | Server peak (vmmap) |
|---|---|---|---|---|---|
| apple (200 DPI, pre-cap) | 16.3 | seed 10 production, 500 pages, 30.8 s, quiet | about 20 s | 503 MB | - |
| apple, with the Vision cap of 2 | `TODO(apple-capped-pages/s)` | pending | pending | pending | - |
| paddleocr-vl (150 DPI) | 0.19 | seed 10 production, 500 pages in 2668 s (5.3 s/page), started after 12:58, one crashed-and-retried attempt not counted. Not a perfectly quiet run: a benchmark agent started near the end | about 27 min | 314 MB | 4.0 GB (4096 MB) |
| dots-ocr (100 DPI) | 0.086 | production pages 1-60, 698 s (11.6 s/page), 13:32-13:44, quiet | about 58 min | 314 MB | 6.8 GB (6758 MB; 7.6 GB in the 200 DPI tune) |
| olmocr | 0.06 | docs/spike.md, quiet (about 16 s/page). My 60-page slice took 2466 s (41 s/page) and was **contended** | about 80 min | 379 MB | 11.2-11.5 GB (11162 MB accept, 11469 MB slice) |

Notes:
- The Apple 300-page benchmark in docs/spike.md (28.4 s, 487 MB, mixed three-page file x 100) is the other quiet reference; the 500-page production here is faster per page (16 pages/s) because it is mostly text-layer-sized pages.
- The Paddle number is slower than the spike's 2.5 s/page because production pages are text-dense and every page also gets the Apple side-pass.
- Eval-set throughput (2-5 page files, one process each) is 2-5 pages/s for Apple and not representative; it is in the `eval/out/results/*.txt` files.

## Targets

Regression guards, set from the accept numbers and rounded to a sensible bound (CER ceilings are about 25% above the measured value; recall floors about 1 point below). They are for the same seeds and the defaults above. Paddle's pleading CER target is loose because of the margin-number effect described above.

| Engine / family | CER <= | Token recall >= | Bates >= |
|---|---|---|---|
| apple pleading | 2.5% | 96.5% | 85% |
| apple contract | 3.0% | 95.5% | - |
| apple fax | 9.0% | 86.5% | - |
| apple table | 0.5% | 97.5% | - |
| apple production | 1.5% | 95.0% | 98.5% |
| apple transcript4up | no target (out of scope) | - | - |
| paddleocr-vl pleading | 16% | 97.5% | 99% |
| paddleocr-vl contract | 0.5% | 99% | - |
| paddleocr-vl fax / table | 0.5% | 99% | - |
| paddleocr-vl transcript4up | no target | 78% | - |
| paddleocr-vl production | 0.5% | 99% | 99% |
| dots-ocr pleading / contract | 1% | 99% | 99% |
| dots-ocr transcript4up | 3% | 98%, line accuracy >= 60% | - |
| dots-ocr fax | 8% | 98% | - |
| dots-ocr table | 0.5% | 99% | - |
| dots-ocr production (60 pages) | 1.5% | 99% | 99% |
| olmocr pleading | 0.5% | 98.5% | 85% (Apple side-pass) |
| olmocr contract / fax | 0.5% | 99% | - |
| olmocr table | 0.5% | 97.5% | - |
| olmocr transcript4up | 15% | 93% | - |
| olmocr production (60 pages) | 0.5% | 99% | 99% |

Where the accept sample is 3 seeds (dots, olmOCR), the targets are looser than the measured values; re-baseline them if the set is enlarged.

## `--legal-cleanup` (contract family, `--keep-furniture`)

Tune seeds 0-9 for Apple, 0-4 for Paddle (this is a feature check, not a tuning decision). Furniture is scored (`--keep-furniture`), so leftover headers and footers cost CER.

| Engine | | CER | Token recall | Furniture strings still in the page body |
|---|---|---|---|---|
| apple (200) | without | 5.4% | 96.2% | 70/96 |
| apple (200) | with | **3.7%** | 96.2% | **6/96** |
| paddleocr-vl (150) | without | 7.7% | 100% | 47/48 |
| paddleocr-vl (150) | with | **5.8%** | 100% | **1/48** |

Furniture goes down sharply and token recall does not drop. (Bates strings are excluded from the count. Exact matching of Apple's misread headers is why 70/96 and not 96/96 are present in the baseline.)

## Pleading paper and tables (Paddle)

Pleading seeds have no tables, so I generated a one-off pleading page with a ruled 3-column table between numbered paragraphs (`eval/out/pleading_table/pleading_table.pdf`, not committed; made with a 40-line reportlab script). Finding for `paddleocr-vl`: **yes, the full-width table crop pulls the margin line numbers into the table.** The recognized table came out with an extra first column holding the margin numbers (rows `| 3 | Invoice No. | Date | Amount |`, `| 4 | INV-4411 | ...`, through `| 8 | TOTAL |`), plus a stray `9` after it. The values themselves were right. So token recall is fine but the table structure is wrong on pleading paper, and the margin numbers are not removable by `--strip-line-numbers`-style scoring. A fix (crop the table to its ruled bounding box instead of a full-width band) is not implemented.

Related Paddle pleading finding: its pleading CER (13.3% accept) is not from misread text. Token recall is 98.7% and most seeds are under 1% CER. The large values come from a few pages: seed 11 page 2 (CER 2.9: a clause is garbled, `Defendant breached the Agreement` came out as `Defendant was the<replacement char> agreement`, and the page also carries extra text) and seed 15 page 2 (CER 0.57: paragraphs 11-14 are missing). On both pages the margin number is glued onto the paragraph numbers (`21.` for `11.`). Neither page was flagged. I did not look at the other 38 pages individually.

## Known gaps

- **Apple Vision segfaults (product bug).** `ocr` crashed with SIGSEGV intermittently in the old binary: first-pass Apple accept had 6 crashes in about 300 scans (contract seeds 18, 28, 34, 39 at 200 DPI; transcript4up seed 28 at 200; pleading seed 22 at 150), and later runs recorded retried crashes: Apple 150 accept pleading seed 38, Apple legal-cleanup contract (2), Paddle tune 150 transcript4up (2), Paddle accept pleading (1), Paddle cleanup (1), Paddle production (1). `eval/run.py` retried them (max 3 attempts) and counts them in the `crashes` column; the retried result is the one reported. The 9 crash reports in `~/Library/Logs/DiagnosticReports/ocr-*.ips` all show the same fault: `objc_retain` inside Apple's `TextRecognition`, an initialization race at process start (within 0.8 s of launch, 6-10 threads inside it). The fix is merged (`3659093`: Vision capped at 2 concurrent requests, plus a supervisor that re-runs a crashed child from its checkpoints) but not measured here.
- Apple Bates accuracy on the accept pleading seeds is 88.6% (100% on the tune seeds); it also limits olmOCR's and dots' Bates when they rely on the side-pass. Not investigated.
- Paddle transcripts and pleading tables (above). Transcript line numbers are unreliable for every engine.
- Paddle pleading pages occasionally lose or repeat paragraphs without being flagged.
- Apple on fax scans is sensitive to DPI (88.5% recall at 200, 96.3% at 150).
- Apple transcripts are out of scope by design and flagged.
- Synthetic fixtures only; no handwriting; no multi-language; skew and noise are one recipe.
- Full 500-page olmOCR and dots runs were not done. Their 300-page times are extrapolations from 60-page slices (dots) or the spike (olmOCR).
- Apple throughput under the Vision cap is pending (placeholder above).
- olmOCR timings in my runs were contended and are not reported as speeds.
- Model-engine accept samples are small (3 seeds for dots and olmOCR).
- The machine slept 00:31-07:45 on 2026-10-08, in the middle of the Paddle tune; tune timings are not used (only accuracy).
