# Model lineup evaluation

## Question

Which local models to keep, add or remove.

## Lineup today

The shipped local engines, from the README "Which engine?" holdout table (hand-checked set: 61 pages of 18 public court filings, 292 values; measured on the Mac):

| Engine | Values found | Verified but wrong | Verified | Amounts | Dates | Dockets | Citations | Transcription CER / word error (12 pages) |
|---|---|---|---|---|---|---|---|---|
| apple | 93.5% | **0** | 141 | 67/71 | 142/151 | 46/49 | 16/19 | 14.3% / 20.7% |
| paddleocr-vl | 96.6% | **0** | 181 | 67/71 | 146/151 | 48/49 | 19/19 | 17.5% / 23.8% |
| dots-ocr | 85.3% | **0** | 175 | 65/71 | 130/151 | 34/49 | 18/19 | 28.1% / 36.4% |
| olmocr | 77.4% | **0** | 160 | 61/71 | 113/151 | 31/49 | 19/19 | 28.7% / 27.6% |
| indra-ocr | 80.8% | **0** | 164 | 65/71 | 117/151 | 34/49 | 18/19 | 21.8% / 38.7% |

Note: on Linux the dots engine runs dots.ocr, not dots.mocr.

## Candidates

From `eval/candidates.json` (pinned revisions, sha256-verified GGUFs):

| Name | Repo | Size | License | Role |
|---|---|---|---|---|
| paddle-raw | PaddlePaddle/PaddleOCR-VL-1.6-GGUF | 0.94 GB | Apache-2.0 | baseline |
| olmocr-raw | bartowski/allenai_olmOCR-2-7B-1025-GGUF | 8.1 GB | Apache-2.0 | baseline |
| dots-ocr-raw | ggml-org/dots.ocr-GGUF | 1.9 GB | MIT | baseline |
| dots-mocr | prithivMLmods/dots.mocr-GGUF | 1.9 GB | MIT (+ supplementary terms) | candidate |
| glm-ocr | ggml-org/GLM-OCR-GGUF | 0.95 GB | MIT | candidate |
| ovisocr3 | mradermacher/OvisOCR3-GGUF | 0.81 GB | Apache-2.0 | candidate |
| lighton-4b | prithivMLmods/LightOnOCR-3-4B-GGUF | 4.5 GB | Apache-2.0 | candidate |

Chandra OCR 2 was skipped: its modified OpenRAIL-M license only covers research, personal use and companies under $2M in funding or revenue.

## Method

- Every model gets one full-page request with its upstream prompt, temperature 0, no Bates corners, no crops, no retries and no filing-stamp restore.
- The baselines go through the same harness, so compare the candidates against the `*-raw` rows, not against the README.
- "Values found" is the measure (off the Mac, `verify_md` has no second reader).
- The raw comparison only screens: replacing a shipped engine needs a full-pipeline comparison in the follow-up plan.
- Filing-stamp dockets and dates are reported apart from the other kinds, because the shipped pipeline restores the stamp.

## Runbook

### R0 — preflight

```sh
ocr server setup --model paddleocr-vl   # installs llama.cpp
# for each name in eval/candidates.json:
python3 eval/candidate_serve.py paddle-raw
python3 eval/candidate_serve.py olmocr-raw
python3 eval/candidate_serve.py dots-ocr-raw
python3 eval/candidate_serve.py dots-mocr
python3 eval/candidate_serve.py glm-ocr
python3 eval/candidate_serve.py ovisocr3
python3 eval/candidate_serve.py lighton-4b
curl -s http://127.0.0.1:8095/v1/models
```

Record any model that fails to load as blocked and move on.

### R1 — baselines

The shipped engines, then the same engines raw through the candidate harness:

```sh
eval/holdout/prepare.py scan --engines paddleocr-vl,dots-ocr,olmocr   # server started per engine
eval/holdout/score.py --engine paddleocr-vl --json eval/out/holdout/paddleocr-vl-linux.json
eval/holdout/score.py --engine dots-ocr --json eval/out/holdout/dots-ocr-linux.json
eval/holdout/score.py --engine olmocr --json eval/out/holdout/olmocr-linux.json

python3 eval/candidate_run.py paddle-raw
python3 eval/candidate_run.py olmocr-raw
python3 eval/candidate_run.py dots-ocr-raw
```

### R2 — candidates

For each candidate name (with a served model from R0):

```sh
python3 eval/candidate_serve.py <name> --gpu 1   # in one terminal
cargo build --release --example verify_md
python3 eval/candidate_run.py <name>             # add --overwrite to redo
python3 eval/holdout/score.py --engine cand-<name> --json eval/out/holdout/cand-<name>.json
```

And the synthetic families:

```sh
python3 eval/vllm_reference.py --candidate <name> --set eval/out/seed10 eval/out/seed11 eval/out/seed12 --out eval/out/candidates --port 8095
```

### R3 — report

The results rows, with filing-stamp dockets and dates split out, CER with and without length-cut pages, and the synthetic families:

```sh
spike/.venv/bin/python eval/candidate_report.py cand-paddle-raw cand-glm-ocr cand-ovisocr3 cand-dots-ocr-raw cand-dots-mocr cand-lighton-4b cand-olmocr-raw paddleocr-vl dots-ocr olmocr
```

A reviewed docket or date counts as stamp-only when its text is on the page's filing-stamp line (`Case/Doc … Filed … Page …`) in the paddle-raw or dots-ocr-raw reading and nowhere else on that page. There are 74 (32 dockets, 42 dates); the other 218 values are "other".

## Gates

**Roles.** The default fast engine is paddle-raw versus glm-ocr and ovisocr3. Layout, transcripts and tables are dots-ocr-raw versus dots-mocr and lighton-4b.

**Adopt (screening) only if all of these hold:**

- holdout values found is at least the baseline + 3 points (about 9 of 292), or within 1 point at ≤ 50% of the baseline's s/page;
- synthetic transcript4up and table CER are no more than 1 point worse than the baseline;
- the license is Apache-2.0 or MIT;
- the GGUF loads in the pinned llama.cpp; and
- the run is complete (61 pages, fewer than 3 failed).

**dots.mocr on Linux:** switch if dots-mocr ≥ dots-ocr-raw on holdout found and within 1 point on every synthetic family.

**olmOCR:** remove in the next release unless olmocr-raw beats every other engine on at least one holdout value kind, or on synthetic table/transcript CER by more than 1 point.

## Results

Run on 2026-10-27, Linux, one RTX 5060 Ti 16 GB (GPU 0), llama.cpp b11510 cuda13, release build of `382798d`. Holdout: 61 pages, 292 values. Synthetic: seeds 10-12, 44 pages. s/page is the sum of request wall times for the raw rows, and the whole `ocr scan` wall time (model load and corner requests included) for the pipeline rows. During the paddle-raw, glm-ocr, ovisocr3, dots-ocr-raw and dots-mocr runs and the paddleocr-vl and dots-ocr pipeline runs, another llama-server was loaded but idle on the same GPUs; lighton-4b, olmocr-raw and the olmocr pipeline ran on a free GPU.

| Name | Found (of 292) | Stamp-only (of 74) | Other (of 218) | Amounts | Dates | Dockets | Citations | CER / word error | s/page | Failed | Length cuts | Synthetic transcript4up CER | Synthetic table CER |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| paddle-raw | 272 (93.2%) | 72 | 200 | 58/71 | 146/151 | 47/49 | 19/19 | 79.1% / 97.0% (9.5% / 13.2% without its length-cut page) | 1.84 | 0 | 1 | 162.8% | 12.3% |
| olmocr-raw | 191 (65.4%) | 4 | 187 | 62/71 | 97/151 | 11/49 | 19/19 | 30.0% / 29.1% | 11.91 | 0 | 2 | 8.3% | 0.0% |
| dots-ocr-raw | 201 (68.8%) | 6 | 195 | 65/71 | 101/151 | 15/49 | 18/19 | 26.8% / 36.3% | 3.62 | 0 | 1 | 2.0% | 0.0% |
| dots-mocr | 211 (72.3%) | 10 | 201 | 66/71 | 110/151 | 15/49 | 18/19 | 19.0% / 27.4% | 4.20 | 0 | 2 | 4.3% | 0.0% |
| glm-ocr | 184 (63.0%) | 6 | 178 | 56/71 | 96/151 | 11/49 | 19/19 | 25.7% / 30.9% | 2.60 | 0 | 1 | 37.7% | 12.3% |
| ovisocr3 | 278 (95.2%) | 74 | 204 | 61/71 | 147/151 | 49/49 | 19/19 | 15.6% / 24.5% | 2.86 | 0 | 1 | 2.0% | 0.0% |
| lighton-4b | 278 (95.2%) | 71 | 207 | 63/71 | 147/151 | 47/49 | 19/19 | 21.2% / 31.1% | 8.83 | 0 | 0 | 72.7% | 0.0% |
| paddleocr-vl (Linux pipeline) | 271 (92.8%) | 71 | 200 | 58/71 | 145/151 | 47/49 | 19/19 | 22.7% / 31.0% | 2.13 | 0 | — | — | — |
| dots-ocr (Linux pipeline) | 206 (70.5%) | 7 | 199 | 67/71 | 104/151 | 15/49 | 18/19 | 17.7% / 24.1% | 4.43 | 0 | — | — | — |
| olmocr (Linux pipeline) | 191 (65.4%) | 4 | 187 | 62/71 | 97/151 | 11/49 | 19/19 | 30.0% / 29.0% | 20.21 | 0 | — | — | — |

No run had a value marked `verified` that is not in the reviewed set. Every pipeline scan exited 0 or 1.

Synthetic CER for every family (in brackets: without the worst page):

| Name | pleading | contract | transcript4up | fax | table | Length cuts |
|---|---|---|---|---|---|---|
| paddle-raw | 50.5% (1.0%) | 0.0% (0.0%) | 162.8% (122.1%) | 0.0% (0.0%) | 12.3% (12.2%) | 3 |
| olmocr-raw | 0.0% (0.0%) | 0.0% (0.0%) | 8.3% (4.6%) | 0.0% (0.0%) | 0.0% (0.0%) | 0 |
| dots-ocr-raw | 0.1% (0.0%) | 0.0% (0.0%) | 2.0% (1.2%) | 1.2% (0.0%) | 0.0% (0.0%) | 0 |
| dots-mocr | 0.0% (0.0%) | 0.0% (0.0%) | 4.3% (1.5%) | 0.7% (0.0%) | 0.0% (0.0%) | 0 |
| glm-ocr | 0.0% (0.0%) | 0.0% (0.0%) | 37.7% (36.3%) | 3.4% (3.0%) | 12.3% (12.2%) | 0 |
| ovisocr3 | 0.0% (0.0%) | 0.0% (0.0%) | 2.0% (1.8%) | 0.0% (0.0%) | 0.0% (0.0%) | 0 |
| lighton-4b | 0.0% (0.0%) | 0.2% (0.2%) | 72.7% (72.5%) | 0.0% (0.0%) | 0.0% (0.0%) | 0 |

Observations:

- The harness tracks the shipped pipeline on this set: paddle-raw 272 vs paddleocr-vl 271, dots-ocr-raw 201 vs dots-ocr 206, olmocr-raw 191 vs olmocr 191.
- Filing stamps: glm-ocr, dots, dots-mocr and olmOCR leave the stamp out (4-10 of 74 found); paddle, ovisocr3 and lighton-4b keep it (71-74). On the Linux pipeline the stamp is not restored for dots and olmOCR either (7 and 4 of 74).
- paddle-raw's transcription CER comes from one page (h12 p2) that repeated until the token limit (15,254 characters against 3,583); the other 11 pages are at 9.5%.
- lighton-4b reads a 4-up transcript page as one HTML table, pairing line n of the left and right panels, so the text is there but the panels are interleaved; hence 73% on every transcript4up page.
- dots-mocr's transcript4up figure is driven by one page (4.3%, 1.5% without it, against dots-ocr-raw's 2.0% and 1.2%).

### Gate verdicts

| Candidate | Role, baseline | Holdout found | Synthetic transcript4up / table | License | Loads | Complete | Verdict |
|---|---|---|---|---|---|---|---|
| glm-ocr | fast, paddle-raw | **fail**: 184 vs 272 (-30.1 points) | pass: 37.7% / 12.3% vs 162.8% / 12.3% | pass (MIT) | pass | pass (61, 0 failed) | not adopted |
| ovisocr3 | fast, paddle-raw | **fail**: 278 vs 272 (+2.1 points, needs +3; 2.86 vs 1.84 s/page, not ≤ 50%) | pass: 2.0% / 0.0% | pass (Apache-2.0) | pass | pass (61, 0 failed) | not adopted (near miss) |
| dots-mocr | layout, dots-ocr-raw | pass: 211 vs 201 (+3.4 points) | **fail**: transcript4up 4.3% vs 2.0% (+2.3 points) | unclear: MIT + supplementary terms (not checked further; the verdict does not depend on it) | pass | pass (61, 0 failed) | not adopted |
| lighton-4b | layout, dots-ocr-raw | pass: 278 vs 201 (+26.4 points) | **fail**: transcript4up 72.7% vs 2.0% | pass (Apache-2.0) | pass | pass (61, 0 failed) | not adopted |

- **dots.mocr on Linux:** holdout 211 ≥ 201 holds, but transcript4up is 2.3 points worse than dots-ocr-raw (every other family is within 1 point). **Do not switch.**
- **olmOCR:** olmocr-raw beats no other engine on any holdout value kind (amounts 62 vs 66 for dots-mocr, dates 97 vs 147, dockets 11 vs 49, citations 19 tied with four others) and is not more than 1 point better on synthetic table (0.0%, tied) or transcript CER (8.3%, worse than 2.0%). It is also the slowest engine (11.9 s/page raw, 20.2 s/page in the Linux pipeline). **Remove in the next release.**

## Decision

By the gates as written:

- **Keep** paddleocr-vl (default fast engine) and dots-ocr, with dots.ocr on Linux.
- **Add** none of glm-ocr, ovisocr3, dots-mocr or lighton-4b.
- **Remove** olmOCR in the next release.
- apple and indra-ocr were not part of this screen and stay.

Not a decision, but worth recording: ovisocr3 was ahead of paddle-raw on every accuracy measure here (holdout +6 values, every stamp value, transcription CER 15.6% vs 9.5-79.1%, synthetic transcripts 2.0% vs 162.8% and tables 0.0% vs 12.3%) and missed only the +3-point holdout margin, at 1.55× the time per page. The gate is kept as written; a later screen that changes it should say so up front.

## Follow-ups

- Remove olmOCR (engine, setup, docs, release notes), as its own plan.
- Switch Linux dots to dots.mocr: not now (gate failed on transcript4up).
- Add an adopted candidate as an engine: none adopted. If ovisocr3 is reconsidered, it needs mlx-vlm support on the Mac and a full-pipeline comparison against paddleocr-vl, as its own plan.
