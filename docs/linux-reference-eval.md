# Linux reference eval (RTX 3090, vLLM, full precision)

**Purpose:** measure whether the quantized MLX builds used on the Mac (8-bit olmOCR and dots, bf16 or 4-bit Paddle) lose accuracy, compared with full-precision models served by vLLM. It uses the same generated pages, the same prompts and the same scorer.

**This is a reference only.** `ocr` itself is macOS-only and does not build on Linux.

Paste the prompt below into Claude Code on the Linux machine after `git clone git@github.com:pkeenan87/ocr.git && cd ocr`.

---

## Prompt

You are running a reference accuracy evaluation for the `ocr` project, which is the current repo. `ocr` is a macOS CLI and cannot be built here. Do not try to build it. You will reproduce its model requests with vLLM on this machine's RTX 3090 (24 GB) and score the results with the repo's own scorer.

**Read first:**
- `eval/README.md`;
- `eval/generate.py` and `eval/score.py`;
- `docs/eval.md`, if present, for the Mac numbers to compare against;
- `docs/spike.md`: the sections on PaddleOCR-VL, olmOCR and dots, for prompts, image sizes and output formats;
- `spike/upstream/*.py`, the verbatim upstream prompts.

### 1. Environment

1. Set up a Python 3.12 venv with `uv`.
2. Install `vllm`, a version that supports Qwen2.5-VL, PaddleOCR-VL and dots.ocr. **Check vLLM's supported-models documentation before choosing the version, and record the version you used.**
3. Install the eval dependencies: `reportlab`, `pypdfium2`, `Pillow`, `rapidfuzz`, `numpy`.
4. Check the GPU with `nvidia-smi`.

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
- pages/s on the 3090;
- peak VRAM.

**Differences from the Mac setup.** These are confounders, so report them:
- pypdfium2 rendering vs CoreGraphics;
- the absent Apple side-pass and table crops;
- full precision vs quantized.

### 6. Write the results to Notion

Create a page titled `ocr reference eval — vLLM full precision (RTX 3090)` with:
- the environment: GPU, driver, CUDA, vLLM version, model ids;
- a per-engine, per-family results table;
- the Mac numbers from `docs/eval.md` alongside, if present;
- the confounders;
- a short conclusion on whether quantization costs accuracy.

Also write the same content to `docs/linux-reference-eval-results.md` and commit it on a branch named `linux-reference-eval`. Do not push to master.

Stop each vLLM server when you are finished with it.
