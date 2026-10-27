#!/usr/bin/env python3
"""Run one raw-harness candidate (eval/candidates.json) over the holdout set,
writing eval/out/holdout/cand-<name>/<id>.md and <id>.raw.json, verifying each
document with verify_md, and writing run.json.

usage:
    python3 eval/candidate_run.py <name> [--port 8095] [--docs h01,h02] [--overwrite]
        [--verify-bin target/release/examples/verify_md]

then score it:
    python3 eval/holdout/score.py --engine cand-<name> --json eval/out/holdout/cand-<name>.json
"""
import argparse
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "holdout"))
import candidates
import prepare


def run_doc(c, prompt, d, out_dir, port, pdfium) -> dict:
    pdf_path = Path(prepare.OUT) / "pdfs" / f"{d['id']}.pdf"
    pdf = pdfium.PdfDocument(str(pdf_path))
    pages = []
    raw = []
    for n in prepare.pages_of(d):
        png = candidates.render(pdf[n - 1], c["render"])
        r = candidates.ask(port, c, png, prompt)
        text = "" if r["error"] else candidates.convert(c, r["reply"])
        pages.append((n, text, r["error"]))
        raw.append(dict(page=n, **r))
    md = out_dir / f"{d['id']}.md"
    md.write_text(candidates.page_markdown(pages))
    (out_dir / f"{d['id']}.raw.json").write_text(json.dumps(raw, indent=1))
    return {"pdf": pdf_path, "md": md, "pages": len(pages),
            "wall_s": sum(x["wall_s"] for x in raw),
            "length_cut": sum(x["finish_reason"] == "length" for x in raw),
            "failed": sum(bool(x["error"]) for x in raw)}


def verify(bin_path, pdf_path, md_path, name):
    r = subprocess.run([str(bin_path), str(pdf_path), str(md_path), "--engine", f"cand-{name}",
                        "--fields", "all"], capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"verify_md failed on {md_path}: {r.stderr.strip()[-400:]}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("name")
    ap.add_argument("--port", type=int, default=8095)
    ap.add_argument("--docs", default="")
    ap.add_argument("--overwrite", action="store_true")
    ap.add_argument("--verify-bin", default=str(ROOT / "target/release/examples/verify_md"))
    a = ap.parse_args()
    if not Path(a.verify_bin).exists():
        raise SystemExit("build it first: cargo build --release --example verify_md")
    c = candidates.load(a.name)
    ids = candidates.server_model_ids(a.port)
    if not any(c["model"]["name"] in i for i in ids):
        raise SystemExit(f"server on port {a.port} serves {ids or 'nothing'}, not {c['model']['name']}; "
                         f"start it with eval/candidate_serve.py {a.name}")
    out_dir = Path(prepare.OUT) / f"cand-{a.name}"
    if any(out_dir.glob("*.md")) and not a.overwrite:
        raise SystemExit(f"{out_dir} already has results; pass --overwrite to replace them")
    if a.overwrite:
        for pat in ("*.md", "*.verify.json", "*.raw.json", "run.json"):
            for f in out_dir.glob(pat):
                f.unlink()
    out_dir.mkdir(parents=True, exist_ok=True)
    prompt = candidates.prompt_of(c)
    import pypdfium2 as pdfium

    docs = prepare.manifest()
    if a.docs:
        docs = [d for d in docs if d["id"] in a.docs.split(",")]
    expected = sum(len(prepare.pages_of(d)) for d in docs)
    pages = wall = length_cut = failed = 0
    for d in docs:
        s = run_doc(c, prompt, d, out_dir, a.port, pdfium)
        verify(a.verify_bin, s["pdf"], s["md"], a.name)
        print(f"cand-{a.name} {d['id']}: {s['pages']} pages, {s['wall_s']:.1f}s, "
              f"length-cut {s['length_cut']}, failed {s['failed']}", flush=True)
        pages += s["pages"]
        wall += s["wall_s"]
        length_cut += s["length_cut"]
        failed += s["failed"]
    run = {"name": a.name, "repo": c["repo"], "revision": c["revision"],
           "model": c["model"]["name"], "mmproj": c["mmproj"]["name"], "server_model_ids": ids,
           "docs": len(docs), "pages": pages, "expected_pages": expected,
           "wall_s": round(wall, 1), "s_per_page": round(wall / pages, 2) if pages else None,
           "length_cut": length_cut, "failed": failed}
    (out_dir / "run.json").write_text(json.dumps(run, indent=1))
    print(f"python3 eval/holdout/score.py --engine cand-{a.name} "
          f"--json eval/out/holdout/cand-{a.name}.json")
    if a.docs:
        print("partial run: score.py needs all 18 documents")
    elif pages != 61:
        print(f"warning: {pages} pages, expected 61")


if __name__ == "__main__":
    main()
