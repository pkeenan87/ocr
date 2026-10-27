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

| Name | Found (of 292) | Amounts | Dates | Dockets | Citations | CER / word error | s/page | Failed | Length cuts | Synthetic transcript4up CER | Synthetic table CER |
|---|---|---|---|---|---|---|---|---|---|---|---|
| paddle-raw | | | | | | | | | | | |
| olmocr-raw | | | | | | | | | | | |
| dots-ocr-raw | | | | | | | | | | | |
| dots-mocr | | | | | | | | | | | |
| glm-ocr | | | | | | | | | | | |
| ovisocr3 | | | | | | | | | | | |
| lighton-4b | | | | | | | | | | | |
| paddleocr-vl (Linux pipeline) | | | | | | | | | | | |
| dots-ocr (Linux pipeline) | | | | | | | | | | | |
| olmocr (Linux pipeline) | | | | | | | | | | | |

## Decision

TBD after the runs.

## Follow-ups

- Switch Linux dots to dots.mocr (if the gate holds), as its own plan.
- Add an adopted candidate as an engine: needs mlx-vlm support on the Mac and a full-pipeline comparison, as its own plan.
- Remove olmOCR (if the gate holds), as its own plan.
