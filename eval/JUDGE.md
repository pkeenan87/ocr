# Jev quality judge (dev only)

`eval/judge.py` asks TypeSafe AI's **Jev** decision model typed questions about each page of an OCR
Markdown output, and returns defect probabilities and a readability score. It is a development
measurement tool for outputs **without ground truth**. It is never part of the `ocr` CLI.
MiniCPM-OCR Jev (OpenBMB, a local region scorer used by `eval/minicpm`) is a different model and stays on this machine. This file is only the TypeSafe judge.

Jev is text-only. It judges whether the text is *plausible*, not whether it matches the page image.

## Privacy rule

Send Jev **only synthetic eval outputs** (`eval/out/`, fixtures from `eval/generate.py`) **or public
documents** approved by the project owner. **Never send client files**, and never point `judge.py` at
real case material. Every page sent leaves the machine (Vercel AI Gateway, then TypeSafe AI or
DigitalOcean).

## How Jev is called

Jev is a *decision* model (Vercel previously called these *evaluation* models). It is **not** served
by Chat Completions, Responses, or the Anthropic/OpenAI-compatible chat endpoints. It has 0 output
tokens of generated text. Instead it returns typed answers.

- Endpoint: `POST https://ai-gateway.vercel.sh/v1/evaluate`
- Headers: `Authorization: Bearer $AI_GATEWAY_API_KEY`, `Content-Type: application/json`
- Body:

```json
{
  "model": "typesafe-ai/jev",
  "state": "The sky is blue.",
  "questions": {
    "garbled": {"type": "boolean", "instructions": "Is the text garbled?",
                "criteria": {"true": "...", "false": "..."}},
    "read": {"type": "score", "instructions": "How readable is the text?",
             "criteria": ["1 unreadable", "2 poor", "3 fair", "4 good", "5 excellent"]},
    "c": {"type": "choice", "instructions": "What colour is the sky?",
          "criteria": {"blue": "blue", "red": "red"}}
  }
}
```

- `state` may be a string, object or array. `questions` is a map of named questions that are all
  answered in **one request**, each evaluated **in isolation** against the same state. So every
  question must carry its own definitions. Shared context in one question is not seen by another.
- Question types: `boolean` (optional `criteria: {true, false}`), `choice` (`criteria`: name to
  description), `score` (`criteria`: array of at least 2 labels, lowest first).
- Response (real test call, abridged):

```json
{"answers": {
   "garbled": {"type": "boolean", "probability": 0.03},
   "read": {"type": "score", "score": 3.98,
            "probabilities": {"0": 0, "1": 0, "2": 0, "3": 0.01, "4": 0.99}, "confidence": 0.99},
   "c": {"type": "choice", "choice": "blue", "probabilities": {"red": 0, "blue": 1}, "confidence": 1}},
 "model": "typesafe-ai/jev",
 "usage": {"inputTokens": 380, "outputTokens": 61},
 "providerMetadata": {"gateway": {"cost": "0.00001596", "finalProvider": "typesafe-ai", "...": "..."}}}
```

- **Score rungs are 0-indexed**: 5 criteria give `score` in 0..4. `judge.py` adds 1 and reports
  readability on 1..5.
- The gateway HTTP API names the boolean answer `probability`. TypeSafe's native API and SDK call
  it `noul`. That native shape is also served at `https://ai-gateway.vercel.sh/typesafe`.
  `judge.py` accepts either.
- Limits: about 32k tokens for state plus questions. `judge.py` chunks pages over 24,000
  characters. It takes the max over chunks for booleans and the mean for readability.
- Latency: about 3 to 8 s per request, so `judge.py` runs pages on 8 threads.
- Cost: listed at $0.042 per 1M input tokens, and output is free. Input tokens track the state
  length plus a small per-question overhead; they are *not* multiplied by the question count. A
  23.6k-character state with 4 questions billed 6,576 input tokens. A typical page with 4–5
  questions is about 1.5–2.5k input tokens, or about $0.0001.
- Chunk size checked: a 23.6k-character state (just under `CHUNK_CHARS`) with all questions
  returned 200.
- No separate TypeSafe account is needed. The AI Gateway key is enough. The gateway routes to
  `typesafe-ai`, with `digitalocean` as the fallback.

Sources:
- Vercel AI Gateway, Decision modality, including the HTTP API and response shape: https://vercel.com/docs/ai-gateway/modalities/decision (formerly `/modalities/evaluation`)
- Vercel changelog, "AI Gateway now supports TypeSafe clients and an HTTP API for Jev": https://vercel.com/changelog/ai-gateway-now-supports-typesafe-clients-and-http-api-for-jev
- Model page: https://vercel.com/ai-gateway/models/jev
- TypeSafe AI docs (primitives Choice / Score / Noul): https://docs.typesafe.ai/introduction

### Credentials

`judge.py` reads `AI_GATEWAY_API_KEY` from the environment, or else by parsing the repo's `.env`.
`.env` is gitignored, mode 600. The key is never printed or logged, and never put in a URL. HTTP
error bodies are redacted before display. Do not `cat` `.env`.

## Usage

```bash
PY="spike/.venv/bin/python -I"
$PY eval/judge.py eval/out/results/apple-accept-dpi200/seed10/contract.md --family contract \
    [--json eval/out/judge/contract.json] [--threshold 0.5]
$PY eval/judge.py --engine-compare \
    eval/out/results/apple-tune-dpi200/seed2/contract.md \
    eval/out/results/dots-tune-dpi150/seed2/contract.md \
    eval/out/results/paddle-tune-dpi150/seed2/contract.md
```

It splits pages on `<!-- page N -->` markers and strips YAML front matter, reusing `score.py`'s
`split_pages`. It then sends one request per page (per chunk) with these questions:

| key | type | asked when |
|---|---|---|
| `garbled` | boolean probability | always |
| `interleaved` | boolean probability | always |
| `repetition` | boolean probability | always |
| `table_broken` | boolean probability | page has ≥2 lines with `\|`, or an HTML table |
| `readability` | score 1–5 | always |

Each question's instructions and `false` criterion explicitly exclude legitimate legal features:
pleading line numbers, Bates numbers, Q./A. lines, numbered sections, citations.

Output: a per-page table, and a per-document summary with the flag rate per question at
`--threshold`, the flagged pages, and the mean readability. `--json` writes everything plus usage.
Responses are cached in `eval/out/judge/cache.jsonl`, keyed by request hash, so reruns are free.
Request, token and cost counters print to stderr. A hard cap of 2,000 requests per process is
enforced.

Default flag thresholds are the calibrated ones below: garbled 0.45, interleaved 0.35, repetition
0.55, table_broken 0.4, and readability < 3.5 listed as low. `--threshold T` overrides all
boolean thresholds.

## Calibration

`eval/judge_calibrate.py [--seeds 10-19] [--results eval/out/results/apple-accept-dpi200] [--dry-run]`

Run on 2026-10-08 with seeds 10–19, all five families, and 149 ground-truth pages. Fixtures and
labels are in `eval/out/judge/injected/`. Results are in `eval/out/judge/calibration.json`, with
per-item answers in `items.jsonl`.

| set | pages | how it is built |
|---|---|---|
| clean | 149 | GT page text as Markdown (GFM tables for the table family), plus furniture: headers, footers, Bates |
| clean_lined | 36 | pleading pages wrapped at 70 chars, with margin numbers 1–28 and a Bates footer |
| interleave | 149 | page wrapped at 45 chars, first and second halves alternated line by line |
| corrupt05/15/30 | 3 × 149 | 5/15/30% of words: rn↔m, l↔1, O↔0, e→c, cl↔d, …, dropped first letter, 15% merged words |
| repetition | 149 | a 40–80 char phrase from the page appended 20× (space- or newline-joined) |
| table_shuffle / drop_pipes / merge | 3 × 20 | GFM table: cells shuffled in ~60% of rows / pipes dropped / every 3rd row merged with the next |
| real | 149 | Apple `apple-accept-dpi200` outputs, per-page CER from `score.score()` (pleading uses `strip_line_numbers`, as `run.py` does) |

### Injected defects (negatives = clean + clean_lined, 185 pages; tables: 20 clean table pages)

| question | AUC | at 0.5: precision / recall | recommended threshold | precision / recall / clean FPR there |
|---|---|---|---|---|
| garbled (all corruption levels) | 1.000 | 1.00 / 0.996 | 0.45 | 1.00 / 0.998 / 0.0% |
| garbled, 5% corruption only | 1.000 | 1.00 / 0.987 | 0.45 | 1.00 / 0.993 / 0.0% |
| interleaved | 0.979 | 1.00 / 0.564 | 0.35 | 0.97 / 0.758 / 1.6% |
| repetition | 1.000 | 1.00 / 1.000 | 0.55 | 1.00 / 1.000 / 0.0% |
| table_broken (all 3 modes) | 1.000 | 1.00 / 1.000 | 0.4 | 1.00 / 1.000 / 0.0% |

Thresholds come from a 0.05 grid, maximizing recall minus FPR subject to a clean FPR ≤ 2%. Ties
go to the middle of the gap.
- Interleave recall at 0.35 varies by family: transcript and fax 100%, pleading 72%, contract
  69%, table 45%. Interleaved contract prose is often still locally fluent.
- Readability as a detector, with AUC against clean for lower scores: interleave 0.93,
  corrupt05 0.99, corrupt15/30 1.00, repetition 1.00, table modes 0.96–1.00.
- Mean readability is clean 4.69, corrupt05 3.37, corrupt30 3.02, interleave 3.88,
  repetition 2.52.
- Cross-firing: questions are not independent. Repetition loops raise garbled (mean 0.62) and
  table_broken. Shuffled tables raise interleaved (0.54). Interleaving prose raises table_broken
  when asked (0.88). Read a flag together with the other answers, not alone.

### False positives on clean legal text at the recommended thresholds

| clean family | n | garbled | interleaved | repetition | table_broken | mean readability |
|---|---|---|---|---|---|---|
| pleading | 36 | 0% | 0% | 0% | – | 4.61 |
| pleading with margin line numbers + Bates | 36 | 0% | 0% | 0% | – | 4.55 |
| contract | 48 | 0% | 0% | 0% | – | 4.92 |
| transcript (Q./A., line numbers) | 25 | 0% | 0% | 0% | – | 4.50 |
| fax letter | 20 | 0% | **15%** | 0% | – | 4.27 |
| table | 20 | 0% | 0% | 0% | 0% | 4.92 |

The fax letterhead, address block and fax header line look interleaved to Jev. At 0.5 the fax
FPR is 0%, but interleave recall drops to 56%. Readability below 3.5 flags 0% of clean pages,
and below 4.0 flags 4.9%.

### Real Apple outputs against measured CER (149 pages)

| subset | n | mean CER | Spearman(readability, CER) | Spearman(garbled, CER) |
|---|---|---|---|---|
| all | 149 | 13.1% | **−0.756** | +0.639 |
| excluding transcript4up | 124 | 3.1% | **−0.587** | +0.486 |
| pleading | 36 | 1.7% | −0.590 | +0.418 |
| contract | 48 | 1.9% | −0.370 | +0.195 |
| table | 20 | 0.2% | −0.715 | +0.739 |
| transcript4up | 25 | 62.6% | −0.334 | +0.240 |
| fax | 20 | 11.5% | +0.580 (wrong sign) | −0.310 |

Detecting bad pages (AUC of low readability):
- CER > 5%: 0.931, or 0.86 excluding transcript4up.
- CER > 10%: 0.905, or 0.71 excluding transcript4up.
- CER > 25%, which is all transcript4up: 0.988.

The interleaved question alone separates CER > 25% with AUC 0.983. The known-bad transcript4up
pages have mean readability 2.58, interleaved flagged on 96% and table_broken on 100%. Apple
wraps them in bogus 2-column tables.

Reading these numbers:
- The injected corruption floor (5% of words) is far denser than real Apple error rates
  (0.2–2% CER on prose), so the injected AUC of 1.0 says nothing about that low-error regime.
  The real-output numbers here are what characterize it.
- The pooled correlation is inflated by the transcript4up cluster, so the honest figure is −0.59
  excluding it.
- The positive fax correlation is a CER artifact, not a judge failure. Fax page 2 is short
  (~420 reference characters), and about 65 characters of unstripped header text inflate its CER
  to ~15%. Page 1 has the same kind of misreads ("Junc", "fittecn", "nonconfonning") and ~5% CER.
  Jev rates both pages ~3.
- At the recommended threshold, **garbled fires on most real Apple pages even at low CER**: 69%
  of contract pages (mean CER 1.9%) and 85% of table pages (0.2%). Spot checks show these are
  real visible misreads: "Assel", "refiigeration", "02124/2024", "Altend", "22" for 2.2, a stray
  margin "15", "April 3,2022". CER barely counts them on a long page. So `garbled` means "this
  page has at least one visible OCR misread". It does not measure how many.
- table_broken fired on every real pleading page where it was asked. Those are true positives:
  Apple wrapped pleading prose and margin numbers in a bogus `| 2 | $89,279.00, … |` table.

## Verdict

**Jev is useful as a dev quality signal for Markdown with no ground truth.** It costs about
$0.0001 per page. It is not a CER substitute. It judges plausibility only, so it cannot see
omitted text, wrong-but-plausible numbers, or anything that requires the page image.

Reliable:
- **repetition**: perfect on injected loops (AUC 1.0) with 0% false positives. Use 0.55. No real
  Apple page was flagged, which is consistent with Apple having no loops.
- **table_broken**: perfect on all three injected break modes, 0% FP on clean tables. Use 0.4.
  On real output it also catches prose wrongly rendered as a table. Expect it to fire on cross-
  defects too, such as interleaving inside a table.
- **garbled**: detects 5% word corruption with AUC 1.0 and 0% FP on clean pleading, transcript
  and contract text, including line numbers, Bates and citations. Use 0.45. Treat it as a binary
  "contains misreads" detector. On a good engine it fires on most pages, so compare **flag rates
  across engines or settings**, not against zero.
- **readability (1–5)**: the best graded signal. Spearman −0.59 with CER on real non-transcript
  pages, and AUC 0.93 for CER > 5%. Flag pages below **3.5**: 0% of clean pages and 85% of real
  pages with CER > 5%. Below 4.0 is more sensitive, with 4.9% clean FP and 35% of real pages with
  CER ≤ 2% flagged. Use mean readability per document for engine comparisons.

Usable with care:
- **interleaved**: AUC 0.98, but probabilities are compressed. Use 0.35 for 76% recall and 1.6%
  overall FP, but 15% FP on fax letterheads and address blocks. Raise it to 0.5 for zero clean
  FP at 56% recall. It strongly flags the real transcript4up quadrant merges.

Not established:
- performance on real scans other than our synthetic families;
- per-page stability (each answer comes from one call, and repeat-call variance was not measured);
- sensitivity to errors that keep text fluent, such as dropped lines or digit swaps in amounts.
  Use `score.py` wherever ground truth exists.

Usage for this calibration run: 1,210 requests including 76 retries (upstream 503s surfaced as
gateway 424 / 5xx), 2.45M input and 87k output tokens, $0.103 gateway cost. Re-running the
analysis is free from the cache.
