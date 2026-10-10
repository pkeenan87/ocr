# Mac `--verify-json` runs for the Azure residual-risk measurement

Apple Vision on the 20 public filings of `eval/public_set.json`, from `eval/verify_eval.py`, copied with `eval/export_verify_bundle.py`. Used by `eval/azure_verify_eval.py` and [docs/azure-verify-eval.md](../../docs/azure-verify-eval.md).

Apple Vision reads differently between macOS versions, so the runs are kept per version, `<os>/<run>/`:

| Folder | Machine | Build | Graded wrong values | Blind `azure_pages` (`all` / `flagged`) |
|---|---|---|---|---|
| `macos26/` | macOS 26.6.2 (home Mac) | master after PR 14 (binary built 12:30, after the merge) | 7 | 222 / 128 |
| `macos27/` | macOS 27.0.1 (work Mac) | master `0a90ec4` | 15 | 218 / 98 (not committed yet: the runs are in the work Mac's `eval/out/verify-latest/`) |

Both builds have the same code. The macOS 27 text is closer to the embedded text on 13 of 14 comparable documents (docs/azure-verify-eval-results.md).

In each run:
- `apple-all/`: `--verify-fields all`; `apple/`: `--verify-fields flagged`.
- `blind/`: the Markdown and report with the embedded text hidden from verify (`OCR_VERIFY_IGNORE_EMBEDDED=1`), as on a scan. Its `azure_pages` are the pages the workflow sends to Azure.
- `truth/`: the report of the same scan with the embedded text, which grades each value.

Nothing is `verified` in any blind run: Apple's two readings are not independent. Pages with an invisible OCR layer are not graded.
