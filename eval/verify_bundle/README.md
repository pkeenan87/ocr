# Mac `--verify-json` runs for the Azure residual-risk measurement

Apple Vision on the 20 public filings of `eval/public_set.json`, from `eval/verify_eval.py` on 2026-10-09 (master after PR 14, macOS 26). Used by `eval/azure_verify_eval.py` and [docs/azure-verify-eval.md](../../docs/azure-verify-eval.md).

- `apple-all/`: `--verify-fields all`; `apple/`: `--verify-fields flagged`.
- `blind/`: the Markdown and report with the embedded text hidden from verify (`OCR_VERIFY_IGNORE_EMBEDDED=1`), as on a scan. Its `azure_pages` are the pages the workflow sends to Azure: 222 of 230 in `all` mode, 128 in `flagged` mode.
- `truth/`: the report of the same scan with the embedded text, which grades each value.

On the Mac, `eval/verify_eval.py` gave for both modes: 167 pages with embedded text, 7 wrong values among the graded ones, all 7 caught as `check` by the blind run, none verified. Pages with an invisible OCR layer are not graded.
