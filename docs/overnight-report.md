# Overnight report (2026-10-07 to 2026-10-08)

## Summary

| Phase | State |
|---|---|
| ✅ 0 Spikes (models, Swift bridge) | done |
| ✅ 1 CLI, config, setup gate | done |
| ✅ 2 Apple engine end to end | done |
| ✅ 2b Bitmap column detection | done |
| ✅ 3 Long-document pipeline | done |
| ✅ 4 Server management | done |
| ✅ 5 Model engines (Paddle, olmOCR, dots) | done |
| ✅ 6 Quality (eval harness, tuned DPI, legal cleanup) | done |

State of the tool: `ocr` scans PDFs and images to Markdown with four engines (Apple Vision by default, plus olmOCR, PaddleOCR-VL and dots-ocr behind local servers), with checkpointed auto-resume, per-engine failure handling, page markers, Bates numbers and opt-in `--legal-cleanup`. On the synthetic eval set all engines meet their targets. On 20 real court filings Apple is clearly the weakest (see "Measured numbers"), and dots-ocr the best but slow. Transcript line numbers are now re-anchored to page geometry for every engine. Apple Vision is capped at 2 concurrent requests to avoid an Apple start-up crash, which makes Apple about 3x slower than unlimited (you kept that). `cargo test --release` was run as a final sanity check (result in the commit report).

## Commits

Phase 0 to 1 (2026-10-07)
- fbf0c4e Phase 0: Swift Vision bridge, model server spike, plan v3
- 6792747 Phase 0 follow-up: vision options, accuracy matrix, real memory numbers
- 3692096 Phase 1: CLI, config, setup gate, input discovery
- 619fffa server start infers the model only when exactly one is installed; plan fixes
- 1263eb5 Fix PLAN table row formatting

Phase 2 and 2b (2026-10-07)
- f547458 Phase 2: Apple Vision engine end to end
- 5b6ded0 Apple defaults to Vision over text layer; narrow currency fix; held-out layout check
- 5aa9fc4 Phase 2b pre-check: Apple Vision fails on transcripts, PaddleOCR-VL exact
- 01abd67 Plan: narrow Phase 2b to contract columns; Paddle before Phase 3
- fa228b7 Phase 2b: bitmap-based column region detection

Phase 4 and 5 (2026-10-07)
- 4c53d1b Phase 4: model server management; Paddle spike via production renderer
- 5822da2 Add transcript, 4-up and narrow-column fixtures for model engine tests
- 8a3d185 Phase 5: PaddleOCR-VL scan engine
- 53e6c28 Plan: record Paddle decisions and the Phase 3 queue-timeout issue
- ff2dc47 Phase 5: olmOCR engine, per-engine ScanEngine trait, generic empty-output handling
- ea37fa4 Phase 5: dots engine (layout JSON to Markdown, crop and prompt_ocr ladder)
- 666a87f Plan: mark Phase 5 done; record olmOCR/dots decisions

Phase 3 (2026-10-07)
- 8bcb0d0 Phase 3: shared pool, per-page checkpoints, auto-resume, request cap, progress
- b4f2515 Phase 3: hide progress bars on TERM=dumb; record benchmark and live --jobs results
- 9e5bae5 Phase 3: clear per-file bars on finish; record paddleocr-vl live --jobs check
- 877f682 Plan: mark Phase 3 done; record pipeline decisions

Phase 6 (2026-10-07 to 2026-10-08)
- dc828ce Phase 6 (checkpoint): eval fixture generators, scorer and runner
- fc68c84 Phase 6: --legal-cleanup strips running headers/footers (opt-in)
- 6b25df7 Plan: record --legal-cleanup decisions
- 570c3f8 Phase 6: eval harness uses isolated state, multi-seed runs, tags and exact micro-averaged counts
- 184f464 Phase 6: retry Apple Vision segfaults (counted), add eval/agg.py family aggregator
- c59c933 Plan: Apple default DPI stays 200 (user decision)
- 0fdb31c Phase 6: README draft (eval numbers pending)
- 2631d47 Phase 6: per-engine default DPI (paddleocr-vl 150, dots-ocr 100); apple stays 200
- 956b650 Phase 6: docs/eval.md, README numbers, --first-pages and furniture counter

Apple Vision start-up crash (2026-10-08)
- bcade8f Plan: Apple Vision capped at 2 concurrent requests (user decision) + supervisor retry
- a076dca Prevent and contain Apple Vision start-up crash: cap Vision at 2 concurrent requests, scan supervisor retries from checkpoints
- 3659093 Merge Apple Vision start-up crash fix (cap 2 + supervisor)

Transcript line-number re-anchoring (2026-10-08)
- 2955395 WIP: transcript line-number re-anchoring (real-model numbers pending)
- 22a3b56 WIP: freeze thresholds (engine coverage 0.5, transcript crop DPI 200) before held-out run
- e6ef7ad Transcripts: re-anchor line numbers to page geometry (all engines)
- de8b78a Merge transcript line-number re-anchoring (all engines)
- 40562ab Update transcript hints after line-number re-anchoring

Dev tooling and reference prompts (2026-10-08)
- 9e95b08 Dev: Jev-based quality judge for eval outputs (calibrated)
- 0a456dc Ignore .env (local AI Gateway key for dev-only eval judge)
- a6cc01b Ignore __pycache__
- 627e35a Dev: public court filing set (20 docs), Apple results and judge spot-checks
- 75a7482 Docs: prompt for the Linux vLLM reference eval
- cc958f9 Docs: prompt for an Azure Document Intelligence comparison on the work machine

Final documentation pass: the commit that adds this report ("Phase 6 done: PLAN, eval placeholders, overnight report").

## What got done

- **Phase 0:** proved the Swift `@_cdecl` bridge to `RecognizeDocumentsRequest` from Rust, and ran all three model engines through `mlx_vlm.server` (docs/spike.md).
- **Phase 1:** full clap command tree, config, setup gate and exit codes.
- **Phase 2:** Apple engine end to end (render, Vision, reading order, currency fix, margin-number filter).
- **Phase 2b:** bitmap XY-cut column detection; 97.6% of lines found on 30 held-out synthetic seeds.
- **Phase 4:** `ocr server setup/start/stop/status`, readiness probe, RAM checks.
- **Phase 5:** PaddleOCR-VL (full page plus table crops), olmOCR (v4 prompt) and dots (layout JSON to Markdown), each with timeout/restart, truncation, loop and empty-output handling and an Apple fallback.
- **Phase 3:** shared request pool, per-page checkpoints, auto-resume, atomic writes, progress bars. Output identical for `--jobs 1` and `--jobs 8`.
- **Phase 6:** eval harness (`eval/`), per-engine DPI tuning, `--legal-cleanup`, README and docs/eval.md, a public-set judge run.
- **Apple Vision crash:** diagnosed an Apple framework start-up race, capped Vision at 2 concurrent requests and added a supervisor that retries from checkpoints.
- **Transcript re-anchoring:** line numbers taken from page geometry for every engine; hints updated.
- **Dev tooling:** Jev judge (Vercel AI Gateway; calibration about $0.10, public run about $0.02; key in gitignored `.env`), 20-filing public set, private GitHub repo https://github.com/pkeenan87/ocr, and the two reference prompts for other machines.

## Measured numbers

### Accuracy per engine (accept seeds, docs/eval.md; synthetic fixtures)

| Engine (DPI) | Family | CER | Token recall | Bates |
|---|---|---|---|---|
| apple (200), 30 seeds | pleading | 1.7% | 97.9% | 88.6% |
| | contract | 2.3% | 96.9% | - |
| | fax | 7.7% | 88.5% | - |
| | table | 0.3% | 98.6% | - |
| | aggregate, no transcripts | 2.32% | 97.4% | 93/105 |
| | production (500 pages) | 1.1% | 96.1% | 99.4% |
| paddleocr-vl (150), seeds 10-19 | pleading | 13.3% | 98.7% | 100% |
| | contract | 0.1% | 100% | - |
| | fax | 0.0% | 100% | - |
| | table | 0.0% | 100% | - |
| | production (500 pages) | 0.0% | 100% | 99.4% |
| dots-ocr (100), seeds 10-12 | pleading | 0.0% | 100% | 100% |
| | contract | 0.2% | 100% | - |
| | fax | 6.3% | 100% | - |
| | table | 0.1% | 100% | - |
| | production (pages 1-60) | 0.6% | 99.8% | 100% |
| olmocr (1288 px), seeds 10-12 | pleading | 0.0% | 99.5% | 90.9% |
| | contract | 0.0% | 100% | - |
| | fax | 0.0% | 100% | - |
| | table | 0.1% | 98.9% | - |
| | production (pages 1-60) | 0.1% | 100% | 100% |

Paddle's pleading CER is inflated by margin line numbers; its token recall is not. Dots and olmOCR rows come from 3 seeds and are indicative. Default DPI per engine: apple 200 (your decision; 150 scored better: tune recall 98.2% vs 97.4%, accept fax 96.3% vs 88.5%), paddleocr-vl 150, dots 100.

### Transcript line numbers, before and after re-anchoring (held-out seeds 40-42, `transcript4up`, lines exact)

| Engine | Before | After |
|---|---|---|
| dots | 68% | 99.9% |
| olmOCR | 30% | 99.9% |
| Apple | 38% | 97.4% |
| paddleocr-vl | 25% | 94.9% |

Paddle misses the 95% target because of its own text errors (dropped, repeated or invented lines). Seeds 10-12 were burned by retuning, so only the 40-42 numbers count.

### Public set (20 CourtListener filings, 230 pages; judged by Jev)

| Engine | Mean readability | Pages < 3.5 | Garbled |
|---|---|---|---|
| dots | 4.39 | 7% | 11% |
| paddleocr-vl | 4.16 | 19% | 28% |
| Apple | 3.57 | 48% | 73% |

- dots took about 2 h for 230 pages (partly contended); Apple about 2 min. One dots document exited 1 (`scan-removed-3`, flagged pages).
- The first 5 dots documents were first run at 200 DPI with an older binary; all 20 were redone at the 100 DPI default (old partial outputs: `eval/out/public/dots-ocr-200dpi-partial`).
- Paddle repeated text on 4% of real pages, below the loop detector's 6-repeat threshold.
- Apple failure modes on real filings (eval/PUBLIC_SET.md): column split on single-column pages cutting lines mid-word; full-page transcripts not detected; pilcrow never read correctly (0/48) and section sign read as S or $ on typewriter scans; Garamond r/t confusion; pleading prose emitted as a table and repeated; `flagged_pages` empty even where the judge rated 67% of deposition pages poor.

### Throughput and memory (300-page scan, quiet machine)

| Engine | Time | `ocr` peak | Server peak |
|---|---|---|---|
| apple, Vision limit 2 | about 90 s (84-96 s; 3.1-3.6 pages/s) | 192 MB (worker) | - |
| paddleocr-vl (150) | about 27 min | 314 MB | 4.0 GB |
| dots-ocr (100) | about 58 min | 314 MB | 6.8 GB |
| olmocr | about 80 min | 379 MB | 11.2 GB |

Paddle comes from a 500-page run, dots from 60 pages, olmOCR from the spike (my runs of it were contended).

### Vision limit and crash test

| Vision limit | 300-page time |
|---|---|
| Unlimited (`OCR_VISION_MAX_CONCURRENT=64`) | 29.0 s |
| 3 | 57.3 s |
| 2 (default) | 84-96 s (about 3x slower) |

All 300 pages were correct at the limit of 2 (case number 200/200, two-column 100/100). The earlier "about 1.36-1.5x" estimate was taken while dots was on the GPU and understated the cost.

| Limit | Launches | Crashes |
|---|---|---|
| Unlimited control | 100 | 6 (6%), codes 139 x5 and 133 |
| 4 | 300 | 7 (2.3%) |
| 2 | 100 + 100 earlier | 0 |

Every real crash happened within 0.81 s of launch. Log: `spike/out/cap4_crash_test.log`; repro: `spike/repro_init_race.sh`.

## Decisions

### Made for you (inform me if you disagree)

- 2026-10-07: model defaults are the newest versions, pinnable with `--model-version`; default output name `<input-stem>.md`; Paddle uses Apple Vision for layout; `--jobs` / `--parallel` added.
- 2026-10-07: `--use-text-layer` defaults to `never` for all engines; no margin-number filter or currency fix on model output; Paddle loop retry at `repetition_penalty 1.15`; every timeout restarts the server.
- 2026-10-08: olmOCR and dots read the full page by default; crops only as a first answer to a loop or truncation. dots never gets `repetition_penalty`. Persistent truncation falls back to Apple and is flagged. olmOCR ignores `--dpi`.
- 2026-10-08: Phase 3 design: checkpoints hold full page results, markers/front matter/`--pages`/`--legal-cleanup` applied at assembly; failed pages retried on re-run; model requests capped at the server's `--max-num-seqs` (1) via `flock`.
- 2026-10-08: `--legal-cleanup` rules (top/bottom 3 lines on at least 50% of pages, fuzzy match at 20+ characters).
- 2026-10-08: per-engine DPI by pre-written rules: paddleocr-vl 150 (recall before CER), dots 100.
- 2026-10-08: transcript hints and docs now recommend dots-ocr; Paddle transcript crops render at no less than 200 DPI.
- 2026-10-08: abandoned parallel stage-B model runs after finding that two servers on one GPU cut combined throughput below half of one.

### Made by you

- 2026-10-08: Apple default DPI stays 200, although 150 scored better (PLAN §8).
- 2026-10-08: Apple Vision limited to 2 concurrent requests plus the supervisor.
- 2026-10-08: kept the limit of 2 a second time, after seeing the corrected 3x cost.
- 2026-10-08: private GitHub repo created and pushed.
- 2026-10-08: Linux reference and Azure comparison prompts requested.

## What was skipped, and why

- **Full 500-page olmOCR and dots production runs:** about 2.2 h and 1.6 h on a quiet machine, and the GPU was shared or the Mac asleep for much of the window (it slept 00:31-07:45, about 7 h lost). 60-page slices were scored instead; the 300-page times are extrapolations.
- **olmOCR on the public filings:** time. Only dots, Paddle and Apple were judged.
- **Handwriting:** no trustworthy ground truth can be generated, so handwritten pages are checked by hand and not scored.
- Also: parallel stage-B runs (GPU contention), Apple Bates investigation, and fixing any of the follow-ups below.

## Follow-ups, prioritized

1. Apple column split firing on single-column real filings (cuts lines mid-word; the main reason Apple is 48% poor on the public set).
2. Paddle pleading-table margin column (full-width crop pulls margin numbers in; crop to the ruled box).
3. Pilcrow and section-sign misreads (Apple).
4. Paddle repetition below the loop-detector threshold on real pages (4% of pages).
5. Apple Bates 88.6% on pleadings.
6. Apple `flagged_pages` never set on bad pages; consider wiring the judge's signals or a heuristic.
7. Pleading margin-number inconsistency across engines.
8. olmOCR rotated-page Bates gap.
9. A text-layer path with layout handling.
10. The flaky `tests/resume.rs::model_directory_output_does_not_depend_on_jobs`, which failed once at server start.
11. Report the Vision crash to Apple via Feedback Assistant, attaching `spike/repro_init_race.sh`.

## Pending on other machines

- **Linux vLLM reference** (RTX 3090 arrives Monday): paste the prompt in [docs/linux-reference-eval.md](linux-reference-eval.md). Expect results on branch `linux-reference-eval`. It measures whether the quantized MLX builds lose accuracy against full precision.
- **Azure Document Intelligence comparison** on the work laptop (about 336 pages, about $3.40 at $10 per 1,000 pages): paste the prompt in [docs/azure-eval.md](azure-eval.md). Expect results on branch `azure-eval`. It needs `.env` with the Azure endpoint and key, created by you, and sends only synthetic fixtures and the 20 public filings.

## Needs your decision or check

- **Apple DPI default:** 150 scored better than 200 (tune recall 98.2% vs 97.4%; accept fax 96.3% vs 88.5%). You decided to keep 200; the goldens are tied to it. Revisit if fax scans matter.
- **Apple on real filings:** 3.57 mean readability and 73% garbled pages. Decide whether Apple should stay the default, or whether to fix follow-up 1 first.
- **Apple cost of the limit of 2:** confirmed twice, noted for completeness.
- **Real-document evidence is thin for olmOCR:** it was never judged on real filings.
- All "Still open" items in PLAN §8 (hardware floor, text-layer trust, Apple legal-number accuracy, privacy) remain open; the throughput item is now filled with measured figures.
- Check that the Paddle 94.9% transcript result (below the 95% target) is acceptable.

## Final verification (2026-10-08 18:27)

- Real-model tests (`OCR_REAL_MODELS=1 cargo test --release --test <engine> -- --ignored`), run one at a time on a quiet machine after the re-anchoring merge and the hint update:

  | Engine | Result | Time |
  |---|---|---|
  | paddle | pass | 49 s |
  | olmOCR | pass | 171 s |
  | dots | pass | 113 s |

- Full suite: 244 passed, 0 failed, 4 ignored (the real-model tests, run separately above).
- No model servers or scans left running.
