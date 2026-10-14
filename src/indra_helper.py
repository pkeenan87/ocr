#!/usr/bin/env python3
"""Runs indra-ocr over chosen pages of one document for `ocr scan --model indra-ocr`.

Installed by `ocr server setup --model indra-ocr` into the state dir and run with the venv's
python. One JSON object per line goes to stdout for every requested page, flushed as soon as the
page is done so the caller can checkpoint it:
  {"page": 1, "markdown": "...", "blocks": [{"label", "bbox", "text"}], "source": "ocr",
   "error": null}
A page that fails has "error" set (and an empty markdown). Page numbers are 1-based.
"""

import argparse
import json
import os
import sys
from pathlib import Path

# The models come from --model-dir only; never let huggingface_hub reach the network.
os.environ["HF_HUB_OFFLINE"] = "1"


def emit(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def failed(number, why):
    emit({"page": number, "markdown": "", "blocks": [], "source": "error", "error": why})


def page_obj(number, page):
    err = None
    if page.source == "error":
        err = page.note or "indra-ocr failed on this page"
    return {
        "page": number,
        "markdown": page.markdown if page.markdown is not None else page.text,
        "blocks": [
            {
                "label": b.get("label"),
                "bbox": [float(v) for v in b.get("bbox", [])],
                "text": b.get("text", ""),
            }
            for b in page.blocks
        ],
        "source": page.source,
        "error": err,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("input")
    ap.add_argument("--pages", required=True, help="1-based, comma separated")
    ap.add_argument("--dpi", type=float, default=200.0)
    ap.add_argument("--threads", type=int, default=4)
    ap.add_argument("--model-dir", required=True)
    ap.add_argument("--password", default=None)
    a = ap.parse_args()
    wanted = [int(p) for p in a.pages.split(",") if p.strip()]

    import indra_ocr
    from indra_ocr.pipeline import is_pdf, read_images

    conv = indra_ocr.load(
        path=a.model_dir,
        threads=max(1, a.threads),
        dpi=a.dpi,
        mode="ocr",
        furniture=True,
        math=False,
    )
    path = Path(a.input)
    try:
        if is_pdf(path):
            import pypdfium2 as pdfium

            doc = pdfium.PdfDocument(a.input, password=a.password)
            try:
                for n in wanted:
                    if not 1 <= n <= len(doc):
                        failed(n, f"page {n} is out of range ({len(doc)} pages)")
                        continue
                    # Converter._safe turns an exception into an empty "error" page.
                    page = conv._safe(lambda: conv.pdf_page(a.input, doc, n - 1))
                    emit(page_obj(n, page))
            finally:
                doc.close()
        else:
            frames = read_images(path)
            for n in wanted:
                if not 1 <= n <= len(frames):
                    failed(n, f"page {n} is out of range ({len(frames)} pages)")
                    continue
                page = conv._safe(lambda: conv.ocr(frames[n - 1]))
                emit(page_obj(n, page))
    except Exception as e:  # noqa: BLE001  (a file that cannot be opened ends the run)
        sys.stderr.write(f"indra-ocr: {type(e).__name__}: {e}\n")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
