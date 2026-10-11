"""Prepare the human-checked holdout set for review (docs/holdout-plan.md).

usage:
    spike/.venv/bin/python -I eval/holdout/prepare.py download
    spike/.venv/bin/python -I eval/holdout/prepare.py scan --engines apple,paddleocr-vl,dots-ocr
    spike/.venv/bin/python -I eval/holdout/prepare.py build

- `download`: the PDFs of eval/holdout/manifest.json into eval/out/holdout/pdfs/ (public
  CourtListener files, spaced 2 s apart, Retry-After honoured; `%PDF-` checked).
- `scan`: each engine reads each document with `--verify-json --verify-fields all`, into
  eval/out/holdout/<engine>/. Model engines need their server running (`ocr server start`).
- `build`: page images (150 DPI PNG) and eval/out/holdout/review.html, a self-contained page that
  shows each page beside the values every reading found. Decisions are kept in the browser and
  exported as JSON; nothing is sent anywhere.

Pre-fill is a convenience, not truth: every value is shown for a human decision, agreed or not.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
MANIFEST = os.environ.get("HOLDOUT_MANIFEST", os.path.join(ROOT, "eval/holdout/manifest.json"))
OUT = os.environ.get("HOLDOUT_OUT", os.path.join(ROOT, "eval/out/holdout"))
PAGE = re.compile(r"<!-- page (\d+)[^>]*-->")
UA = "ocr-eval/0.1 (personal research)"

# Pages that also get a full transcription (diverse kinds, about a fifth of the set).
TRANSCRIBE = {("h01", 1), ("h02", 2), ("h03", 1), ("h05", 1), ("h08", 2), ("h09", 1),
              ("h11", 2), ("h12", 2), ("h13", 2), ("h14", 2), ("h16", 1), ("h18", 2)}


def manifest():
    with open(MANIFEST, encoding="utf-8") as f:
        return json.load(f)["documents"]


def pages_of(d):
    if not d.get("pages_arg"):
        return list(range(1, d["n_pages_used"] + 1))
    a, b = d["pages_arg"].split("-")
    return list(range(int(a), int(b) + 1))


def download(_):
    os.makedirs(f"{OUT}/pdfs", exist_ok=True)
    for d in manifest():
        dest = f"{OUT}/pdfs/{d['id']}.pdf"
        if os.path.exists(dest):
            continue
        data = b""
        for _ in range(5):
            try:
                req = urllib.request.Request(d["pdf_url"], headers={"User-Agent": UA})
                with urllib.request.urlopen(req, timeout=60) as r:
                    data = r.read()
                break
            except urllib.error.HTTPError as e:
                if e.code != 429:
                    raise
                time.sleep(int(e.headers.get("Retry-After", "6")) + 1)
        if not data.startswith(b"%PDF-"):
            sys.exit(f"{d['id']}: not a PDF")
        with open(dest, "wb") as f:
            f.write(data)
        print(f"{d['id']}: {len(data) // 1024} KB")
        time.sleep(2)


def scan(a):
    ocr = a.ocr or os.path.join(ROOT, "target/release/ocr")
    for engine in a.engines.split(","):
        os.makedirs(f"{OUT}/{engine}", exist_ok=True)
        for d in manifest():
            md = f"{OUT}/{engine}/{d['id']}.md"
            args = [ocr, "scan", "--input-file", f"{OUT}/pdfs/{d['id']}.pdf", "--output-file", md,
                    "--model", engine, "--overwrite", "--quiet", "--verify-json",
                    "--verify-fields", "all"]
            if d.get("pages_arg"):
                args += ["--pages", d["pages_arg"]]
            r = subprocess.run(args, capture_output=True, text=True)
            print(f"{engine} {d['id']}: exit {r.returncode}", flush=True)
            if r.returncode not in (0, 1):
                print(r.stderr[-400:], file=sys.stderr)


def key(f):
    v = f.get("value")
    return (f["kind"], str(v) if v is not None else re.sub(r"[^0-9a-z§¶$%.-]", "", f["text"].lower()))


def build(a):
    import pypdfium2 as pdfium
    engines = a.engines.split(",")
    os.makedirs(f"{OUT}/pages", exist_ok=True)
    pages = []
    for d in manifest():
        pdf = pdfium.PdfDocument(f"{OUT}/pdfs/{d['id']}.pdf")
        reports, bodies = {}, {}
        for e in engines:
            try:
                with open(f"{OUT}/{e}/{d['id']}.verify.json", encoding="utf-8") as f:
                    reports[e] = {p["page"]: p for p in json.load(f)["pages"]}
                with open(f"{OUT}/{e}/{d['id']}.md", encoding="utf-8") as f:
                    md = f.read()
                parts = PAGE.split(md.split("\n---\n", 1)[-1])
                bodies[e] = {int(parts[i]): parts[i + 1].strip() for i in range(1, len(parts) - 1, 2)}
            except FileNotFoundError:
                pass
        for n in pages_of(d):
            img = f"pages/{d['id']}-p{n}.png"
            if not os.path.exists(f"{OUT}/{img}"):
                pdf[n - 1].render(scale=150 / 72).to_pil().save(f"{OUT}/{img}")
            values = {}
            for e, rep in reports.items():
                for fld in rep.get(n, {}).get("fields", []):
                    k = key(fld)
                    v = values.setdefault(k, {"kind": fld["kind"], "texts": {}, "line": fld["line"]})
                    v["texts"].setdefault(e, fld["text"])
            vals = sorted(values.values(), key=lambda v: (v["line"], v["kind"]))
            for v in vals:
                v["agreed"] = len(v["texts"]) == len(reports)
            hints = {}
            for e, rep in reports.items():
                p = rep.get(n, {})
                hints[e] = sorted(set(p.get("page_check", [])) | ({"annotations"} if p.get("annotations") else set()))
            prefill = next((bodies[e].get(n, "") for e in ("dots-ocr", "paddleocr-vl", "apple") if e in bodies), "")
            pages.append(dict(doc=d["id"], page=n, what=d["what"], image=img, values=vals,
                              hints=hints, transcribe=(d["id"], n) in TRANSCRIBE,
                              transcript_prefill=prefill if (d["id"], n) in TRANSCRIBE else ""))
    with open(os.path.join(ROOT, "eval/holdout/review_template.html"), encoding="utf-8") as f:
        page = f.read()
    data = json.dumps({"engines": engines, "pages": pages}, ensure_ascii=False)
    page = page.replace("/*DATA*/null", data.replace("</", "<\\/"))
    with open(f"{OUT}/review.html", "w", encoding="utf-8") as f:
        f.write(page)
    print(f"{len(pages)} pages -> {OUT}/review.html")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("step", choices=["download", "scan", "build"])
    p.add_argument("--engines", default="apple,paddleocr-vl,dots-ocr")
    p.add_argument("--ocr")
    a = p.parse_args()
    {"download": download, "scan": scan, "build": build}[a.step](a)


if __name__ == "__main__":
    main()
