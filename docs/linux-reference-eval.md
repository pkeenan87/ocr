# Linux reference eval (NVIDIA GPU, vLLM, full precision)

**Purpose:** measure whether the quantized MLX builds used on the Mac (8-bit olmOCR and dots, bf16 or 4-bit Paddle) lose accuracy, compared with full-precision models served by vLLM. It uses the same generated pages, the same prompts and the same scorer.

**This is a reference only.** It measures the models, not `ocr`: since Phase 7 `ocr` builds and runs on Linux with llama.cpp, and its own Linux numbers come from `eval/run.py` (PLAN.md §9.7).

**Last run:** 2026-10-14 on an RTX 5060 Ti (16 GB) with vLLM 0.31.0. Results: `docs/linux-reference-eval-results.md` on branch `linux-reference-eval`. The harness from that run is `eval/vllm_reference.py` on the same branch; reuse it rather than writing a new one.

Paste the prompt below into Claude Code on the Linux machine, in a clone of the repo.

---

> **Transcript line numbers: compare raw against raw.** Since 2026-10-08 the Mac's `ocr` re-anchors transcript line numbers to page geometry, using Apple Vision line positions (`src/anchor.rs`), for every engine. That is post-processing which the vLLM run does not get. For **transcript-line accuracy**, compare against the Mac's **before re-anchoring** numbers, the raw engine output:
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

You are running a reference accuracy evaluation for the `ocr` project, which is the current repo. You will reproduce its model requests with vLLM on this machine's NVIDIA GPU and score the results with the repo's own scorer. You do not need to build `ocr` for this. If branch `linux-reference-eval` exists, start from its `eval/vllm_reference.py` harness.

**Read first:**
- `eval/README.md`;
- `eval/generate.py` and `eval/score.py`;
- `docs/eval.md`, if present, for the Mac numbers to compare against;
- `docs/spike.md`: the sections on PaddleOCR-VL, olmOCR and dots, for prompts, image sizes and output formats;
- `spike/upstream/*.py`, the verbatim upstream prompts.

### 1. Environment

1. Set up a Python 3.12 venv with `uv`.
2. Install `vllm`, a version that supports Qwen2.5-VL, PaddleOCR-VL and dots.ocr. **Check vLLM's supported-models documentation before choosing the version, and record the version you used.** 0.31.0 worked on Blackwell (sm_120). Also install `ninja` into the venv and put the venv's `bin` on `PATH`: vLLM's warm-up compile fails without it.
3. Install the eval dependencies: `reportlab`, `pypdfium2`, `Pillow`, `rapidfuzz`, `numpy`.
4. Check the GPU with `nvidia-smi`. Note its memory: with 16 GB, olmOCR must use the FP8 build (bf16 needs about 17 GB).

### 2. Fixtures

Generate seeds 10–12 with:
```bash
python eval/generate.py --seed N --out eval/out/seedN
```
These are the Mac's accept seeds for olmOCR and dots, so the results are comparable.

Skip the `production` family, except for a `--pages 1-60` slice of seed 10 if time allows.

### 3. Engines

Serve one engine at a time, in this order:

| Engine | Model | Prompt | Image | Output conversion |
|---|---|---|---|---|
| PaddleOCR-VL | `PaddlePaddle/PaddleOCR-VL-1.6` (bf16) | `OCR:` on the full page | Render with pypdfium2 at the Mac's tuned DPI (see `docs/eval.md`; 150 if absent) | none |
| olmOCR | `allenai/olmOCR-2-7B-1025` (bf16; use the official FP8 build if bf16 does not fit) | Verbatim `build_no_anchoring_v4_yaml_prompt()`, **text before the image** | Longest side 1288 px | Strip the YAML front matter. Convert HTML tables to `cell \| cell` rows |
| dots | `rednote-hilab/dots.mocr` | Verbatim `prompt_layout_all_en` | Render at the DPI in `docs/eval.md` | Parse the JSON array; join `text` fields in order. Title/Section-header as headings, HTML tables as rows, keep Page-header and Page-footer text |

For all engines use `temperature 0`. Use `max_tokens` 4096 for PaddleOCR-VL and olmOCR, and 8192 for dots.

**Paddle tables:** PaddleOCR-VL's table crops depend on Apple Vision table detection, which isn't available here. Use full-page `OCR:` only and note this difference.

### 4. Output for the scorer

For each fixture, write `<family>.md` with one `<!-- page N -->` marker per page followed by that page's converted text, so that `eval/score.py` can split it. Put no Bates numbers in the markers; Bates accuracy is not comparable here. Then score with the **same flags** `docs/eval.md` says the Mac runs used.

### 5. Measurements

For each engine and family, record:
- CER;
- token recall;
- transcript-line accuracy;
- pages/s on the GPU;
- peak VRAM.

**Differences from the Mac setup.** These are confounders, so report them:
- pypdfium2 rendering vs CoreGraphics;
- the absent Apple side-pass and table crops;
- full precision vs quantized.

- **Transcript lines:** compare against the Mac's *raw* (before re-anchoring) numbers, not the re-anchored ones. See the "compare raw against raw" note at the top of `docs/linux-reference-eval.md` for the table and the reason.

### 6. Write the results to Notion

Create a page titled `ocr reference eval — vLLM full precision (<GPU>)` with:
- the environment: GPU, driver, CUDA, vLLM version, model ids;
- a per-engine, per-family results table;
- the Mac numbers from `docs/eval.md` alongside, if present;
- the confounders;
- a short conclusion on whether quantization costs accuracy.

Also write the same content to `docs/linux-reference-eval-results.md` and commit it on a branch named `linux-reference-eval`. Do not push to master.

Stop each vLLM server when you are finished with it.
