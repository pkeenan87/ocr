# ocr reference eval — vLLM full precision (RTX 5060 Ti)

Run 2026-10-14 by following `docs/linux-reference-eval.md`. The runbook was written for an RTX 3090 (24 GB). This run used a 16 GB card, so olmOCR ran as the official FP8 build instead of bf16. The harness is `eval/vllm_reference.py`; raw replies and per-file Markdown are in `eval/out/vllm/` (not committed).

**Question:** do the quantized builds `ocr` uses lose accuracy against the models at full precision?

**Answer: no measurable loss.** On every engine and family, the full-precision reference and the quantized engines agree to within about 1 point of CER, often exactly. The differences that remain come from the pipeline (crops, the Vision side-pass, Bates corner requests), not from quantization.

## Environment

| | |
|---|---|
| GPU | NVIDIA GeForce RTX 5060 Ti, 16 GB (Blackwell, sm_120) |
| Driver | 610.57.04; `nvidia-smi` reports "CUDA UMD Version: 13.3" |
| vLLM | 0.31.0 (torch 2.13.0+cu132), Python 3.12, plus `ninja` (needed by the warm-up compile) |
| Rendering | pypdfium2 5.14.0 |
| Fixtures | `eval/generate.py`, accept seeds 10–12, families pleading, contract, transcript4up, fax, table (production skipped) |
| Scoring | `eval/score.py`, the same flags as `eval/run.py`: `--strip-line-numbers` on pleading only, ground-truth furniture ignored |

| Engine | Model (HF revision) | Precision | Prompt | Image | max_tokens |
|---|---|---|---|---|---|
| PaddleOCR-VL | `PaddlePaddle/PaddleOCR-VL-1.6` (`c5630ab`) | bf16 | `OCR:`, full page | 150 DPI | 4096 |
| olmOCR | `allenai/olmOCR-2-7B-1025-FP8` (`40bd720`) | FP8 (bf16 needs about 17 GB) | `build_no_anchoring_v4_yaml_prompt()`, text before the image | longest side 1288 px | 4096 |
| dots | `rednote-hilab/dots.mocr` (`e539fbb`) | bf16 | `prompt_layout_all_en` | 100 DPI | 8192 |

All requests use temperature 0, one at a time. Conversions: olmOCR's YAML front matter is stripped; dots' JSON is joined in order (Title as `#`, Section-header as `##`, Picture dropped, Page-header and Page-footer kept). HTML tables are left to the scorer, which removes table markup.

## Results

CER / token recall. Transcript lines are the raw engine output (no re-anchoring; see the runbook note).

| Engine | Family | Pages | vLLM full precision | `ocr` llama.cpp CUDA (GGUF) | Mac `ocr` (MLX), docs/eval.md |
|---|---|---|---|---|---|
| PaddleOCR-VL | pleading | 11 | 0.9% / 100% | 0.9% / 98.4% | 13.3% / 98.7% (seeds 10–19) |
| | contract | 14 | 0.0% / 100% | 0.0% / 100% | 0.1% / 100% |
| | transcript4up | 9 | 69.2% / 78.3%, lines 22.1% | 39.1% / 91.5%, lines 34.3% | 51.1% / 81.5%, lines 25% raw |
| | fax | 6 | 0.0% / 100% | 0.0% / 100% | 0.0% / 100% |
| | table | 6 | 12.3% / 100% | 12.3% / 100% | 0.0% / 100% |
| olmOCR | pleading | 11 | 0.0% / 94.0% | 0.0% / 98.9% | 0.0% / 99.5% |
| | contract | 14 | 0.0% / 100% | 0.0% / 100% | 0.0% / 100% |
| | transcript4up | 9 | 11.7% / 96.9%, lines 26.6% | 11.6% / 96.9%, lines 25.6% | 11.2% / 96.9%, lines 30% raw |
| | fax | 6 | 0.0% / 100% | 0.0% / 100% | 0.0% / 100% |
| | table | 6 | 0.0% / 100% | 0.0% / 100% | 0.1% / 98.9% |
| dots | pleading | 11 | 0.0% / 100% | 0.1% / 99.5% (dots.ocr) | 0.0% / 100% |
| | contract | 14 | 0.2% / 100% | 0.0% / 100% | 0.2% / 100% |
| | transcript4up | 9 | 1.0% / 99.2%, lines 71.2% | 2.0% / 100%, lines 76.3% | 1.2% / 100%, lines 68% raw |
| | fax | 6 | 5.6% / 100% | 1.2% / 100% | 6.3% / 100% |
| | table | 6 | 0.0% / 100% | 0.0% / 100% | 0.1% / 100% |

Quantization in each column: vLLM as in the environment table. llama.cpp: PaddleOCR-VL f16, olmOCR Q8_0, dots **dots.ocr** Q8_0 (dots.mocr has no GGUF). Mac: PaddleOCR-VL bf16, olmOCR 8-bit, dots.mocr 8-bit.

**Speed and memory on the 5060 Ti** (pages/s across families; one request at a time):

| Engine | vLLM | llama.cpp CUDA | Weights loaded (vLLM) | GPU memory in use, llama.cpp |
|---|---|---|---|---|
| PaddleOCR-VL | 0.25–1.02 | 0.16–0.71 | 1.8 GB | 2.4 GB |
| olmOCR | 0.03–0.16 | 0.03–0.11 | 9.6 GB | 10.2 GB |
| dots | 0.03–0.16 | 0.13–0.43 | 5.9 GB | 4.4 GB |

vLLM preallocates a fixed fraction of the card (11–13.5 GB peak observed at `--gpu-memory-utilization` 0.8–0.9), so its peak is not a need. llama.cpp's figure is what `ocr server status` reports. The `ocr` timings include rendering and Bates corner requests; vLLM's are model time only.

## Reading the differences

- **olmOCR, Mac 8-bit vs FP8:** identical CER on four families; transcripts 11.2% vs 11.7%. The lower pleading token recall under vLLM (94.0%) is exactly the four to five Bates numbers per file: olmOCR drops footers, and `ocr` recovers them with its corner requests, which the reference run does not make.
- **dots, Mac dots.mocr 8-bit vs dots.mocr bf16:** within 0.7 points everywhere (fax 6.3% vs 5.6%, transcripts 1.2% vs 1.0%). No quantization loss.
- **PaddleOCR-VL** runs at bf16/f16 in every column, so it has no quantization question. Its differences are the pipeline:
  - tables: the Mac crops tables found by Vision (`Table Recognition:`), which takes CER to 0.0%; full-page `OCR:` gives 12.3% under both vLLM and llama.cpp;
  - transcripts: `ocr` splits 4-up pages into panel crops, which helps (39.1% on llama.cpp), while full-page `OCR:` under vLLM gives 69.2%;
  - pleading: the Mac's 13.3% is over seeds 10–19 with Vision-driven column and band crops; full-page `OCR:` is at 0.9% on seeds 10–12.

## Confounders

- **Rendering:** pypdfium2 (this run) vs CoreGraphics (Mac). `ocr` on Linux renders with pdfium too.
- **No Apple side-pass:** no Vision table crops, Bates from Vision, or transcript detection in the reference run. Bates accuracy is not compared.
- **Precision:** olmOCR is FP8 here, not bf16 (the card has 16 GB). FP8 is closer to the Mac's 8-bit than bf16 would be, so the olmOCR comparison is strictly "8-bit MLX vs FP8".
- **dots model:** `ocr` off the Mac uses dots.ocr (GGUF); the reference and the Mac use dots.mocr.
- **Small sample:** 46 pages per engine (seeds 10–12), as in the Mac's olmOCR and dots accept sets.

## Notes for the runbook

- `docs/linux-reference-eval.md` says `ocr` does not build on Linux. That is stale since Phase 7: `ocr` builds and runs on Linux, and its own CUDA results are the middle column above.
- vLLM 0.31.0 needs `ninja` on `PATH` for its warm-up compile; install it into the venv.
