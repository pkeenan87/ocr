#!/usr/bin/env python3
"""Reference run of the model engines at full precision through vLLM (docs/linux-reference-eval.md).

Reproduces each engine's full-page request with the upstream prompt, writes `<family>.md` with one
`<!-- page N -->` marker per page (no Bates), and scores it with eval/score.py using the same flags
as eval/run.py. One vLLM server must already be serving the engine's model on --port.

    python eval/vllm_reference.py --engine dots --set eval/out/seed10 eval/out/seed11 \
        --out eval/out/vllm --model rednote-hilab/dots.mocr

    python eval/vllm_reference.py --candidate glm-ocr --set eval/out/seed10 eval/out/seed11 eval/out/seed12 --out eval/out/candidates --port 8095

--rescore scores the Markdown a previous run saved under --out, with no server (no timings or GPU memory).
"""
import argparse, base64, concurrent.futures as cf, io, json, re, subprocess, sys, time, urllib.request
from pathlib import Path

import pypdfium2 as pdfium

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "eval"))
sys.path.insert(0, str(ROOT / "spike/upstream"))
import score as scorer  # noqa: E402
import candidates  # noqa: E402
from allenai_olmocr import build_no_anchoring_v4_yaml_prompt  # noqa: E402

FAMILIES = ["pleading", "contract", "transcript4up", "fax", "table"]


def upstream_prompt(file, key):
    src = (ROOT / "spike/upstream" / file).read_text()
    return re.search(rf'"{key}":\s*"""(.*?)"""', src, re.S).group(1)


# Engine: (prompt, text first, render, max_tokens). Render: ("dpi", n) or ("side", px).
ENGINES = {
    "paddle": ("OCR:", False, ("dpi", 150), 4096),
    "olmocr": (build_no_anchoring_v4_yaml_prompt(), True, ("side", 1288), 4096),
    "dots": (upstream_prompt("rednote-hilab_dots.mocr.py", "prompt_layout_all_en"), False, ("dpi", 100), 8192),
}


def render(page, how):
    kind, n = how
    w, h = page.get_size()
    scale = n / 72 if kind == "dpi" else n / max(w, h)
    buf = io.BytesIO()
    page.render(scale=scale).to_pil().convert("RGB").save(buf, "PNG")
    return buf.getvalue()


def ask(port, model, png, prompt, text_first, max_tokens):
    image = {"type": "image_url", "image_url": {"url": "data:image/png;base64," + base64.b64encode(png).decode()}}
    text = {"type": "text", "text": prompt}
    body = {"model": model, "temperature": 0, "max_tokens": max_tokens,
            "messages": [{"role": "user", "content": [text, image] if text_first else [image, text]}]}
    req = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions", json.dumps(body).encode(),
                                 {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=900) as r:
        return json.load(r)["choices"][0]["message"]["content"] or ""


def strip_yaml(reply):
    m = re.match(r"\s*---\n.*?\n---\n?", reply, re.S)
    return reply[m.end():] if m else reply


def dots_markdown(reply):
    t = reply.strip()
    t = re.sub(r"^```(?:json)?\s*|\s*```$", "", t)
    try:
        items = json.loads(t)
    except json.JSONDecodeError:
        return reply  # unparseable (e.g. truncated): scored as is
    parts = []
    for el in items if isinstance(items, list) else []:
        cat, txt = el.get("category", ""), (el.get("text") or "").strip()
        if cat == "Picture" or not txt:
            continue
        parts.append({"Title": "# ", "Section-header": "## "}.get(cat, "") + txt)
    return "\n\n".join(parts)


CONVERT = {"paddle": lambda r: r, "olmocr": strip_yaml, "dots": dots_markdown}


def vram_mb():
    try:
        out = subprocess.run(["nvidia-smi", "--query-gpu=memory.used", "--format=csv,noheader,nounits"],
                             capture_output=True, text=True).stdout
        return sum(int(x) for x in out.split())
    except OSError:
        return None


def main():
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--engine", choices=ENGINES)
    g.add_argument("--candidate")
    ap.add_argument("--model", help="model id as served by vLLM")
    ap.add_argument("--set", required=True, nargs="+", dest="set_dirs")
    ap.add_argument("--out", required=True)
    ap.add_argument("--port", type=int, default=8000)
    ap.add_argument("--families", default=",".join(FAMILIES))
    ap.add_argument("--first-pages", type=int)
    ap.add_argument("--concurrency", type=int, default=1)
    ap.add_argument("--rescore", action="store_true", help="score the saved Markdown; no server needed")
    a = ap.parse_args()
    if a.engine and not a.model:
        ap.error("--model is required with --engine")
    if a.candidate:
        c = candidates.load(a.candidate)
        prompt = candidates.prompt_of(c)
        how = c["render"]
        tag = f"cand-{a.candidate}"
    else:
        c = None
        prompt, text_first, how, max_tokens = ENGINES[a.engine]
        tag = a.engine
    out_root = Path(a.out) / tag
    results, peak_vram = {}, 0
    for set_dir in map(Path, a.set_dirs):
        for fam in a.families.split(","):
            pdf_path = set_dir / f"{fam}.pdf"
            if not pdf_path.exists():
                continue
            gt = json.load(open(set_dir / f"{fam}.gt.json"))
            pdf = pdfium.PdfDocument(str(pdf_path))
            n = len(pdf) if not a.first_pages else min(a.first_pages, len(pdf))
            if a.first_pages:
                gt = {**gt, "pages": gt["pages"][:n]}
            dest = out_root / set_dir.name / f"{fam}.md"
            if a.rescore:
                md, wall = dest.read_text(), None
                if c is not None:
                    raw_path = dest.with_suffix(".raw.json")
                    asks = json.loads(raw_path.read_text()) if raw_path.exists() else []
                    failed = sum(bool(x["error"]) for x in asks)
                    length_cut = sum(x["finish_reason"] == "length" for x in asks)
            else:
                if c is not None:
                    pngs = [candidates.render(pdf[i], c["render"]) for i in range(n)]
                else:
                    pngs = [render(pdf[i], how) for i in range(n)]
                t0 = time.time()
                with cf.ThreadPoolExecutor(a.concurrency) as pool:
                    if c is not None:
                        asks = list(pool.map(lambda p: candidates.ask(a.port, c, p, prompt), pngs))
                        replies = ["" if x["error"] else x["reply"] for x in asks]
                        failed = sum(bool(x["error"]) for x in asks)
                        length_cut = sum(x["finish_reason"] == "length" for x in asks)
                        md = "".join(f"<!-- page {i} -->\n\n{candidates.convert(c, r).strip()}\n\n" for i, r in enumerate(replies, 1))
                        raw = json.dumps(asks, indent=1)
                    else:
                        replies = list(pool.map(lambda p: ask(a.port, a.model, p, prompt, text_first, max_tokens), pngs))
                        md = "".join(f"<!-- page {i} -->\n\n{CONVERT[a.engine](r).strip()}\n\n" for i, r in enumerate(replies, 1))
                        raw = json.dumps(replies, indent=1)
                wall = time.time() - t0
                peak_vram = max(peak_vram, vram_mb() or 0)
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_text(md)
                (dest.with_suffix(".raw.json")).write_text(raw)
            s = scorer.score(gt, md, strip_line_numbers=(fam == "pleading"))
            e = {k: s[k] for k in ("dist_sum", "len_sum", "tokens_found", "tokens_total", "tl_ok", "tl_total",
                                   "cer_pages", "dist_pages", "len_pages")}
            e.update(pages=n, wall_s=None if wall is None else round(wall, 2), cer=s["cer"])
            if c is not None:
                e["failed"] = failed
                e["length_cut"] = length_cut
            results.setdefault(fam, []).append(e)
            took = "" if wall is None else f", {wall:.1f}s"
            print(f"{set_dir.name}/{fam}: {s['cer']} CER{took}, pages {s['cer_pages']}", file=sys.stderr, flush=True)
    summary = {}
    for fam, es in results.items():
        g = lambda k: sum(e[k] for e in es)
        r = lambda x, y: round(g(x) / g(y), 4) if g(y) else None
        # Pooled CER without the single worst page, to see how much one page (e.g. a loop) carries.
        per_page = sorted(((d, l) for e in es for d, l in zip(e["dist_pages"], e["len_pages"])),
                          key=lambda dl: dl[0] / max(dl[1], 1))[:-1]
        rest = sum(l for _, l in per_page)
        walls = [e["wall_s"] for e in es]
        summary[fam] = {"pages": g("pages"), "cer": r("dist_sum", "len_sum"),
                        "cer_excl_worst": round(sum(d for d, _ in per_page) / rest, 4) if rest else None,
                        "token_recall": r("tokens_found", "tokens_total"),
                        "transcript_line_accuracy": r("tl_ok", "tl_total"),
                        "pages_per_s": None if None in walls else round(g("pages") / sum(walls), 3)}
        if c is not None:
            summary[fam].update(failed=g("failed"), length_cut=g("length_cut"))
    res = {"engine": tag, "model": a.model or c["model"]["name"], "render": how, "peak_vram_mb": peak_vram,
           "runs": results, "summary": summary}
    Path(a.out).mkdir(parents=True, exist_ok=True)
    json.dump(res, open(Path(a.out) / f"{tag}{'-rescore' if a.rescore else ''}.json", "w"), indent=1)
    f = lambda v: "-" if v is None else f"{v * 100:.1f}%"
    print(f"\n{tag} ({a.model or c['model']['name']})  peak GPU memory used {peak_vram} MB")
    print("| family | pages | CER | CER without worst page | token recall | transcript lines | pages/s |\n"
          "|---|---|---|---|---|---|---|")
    for fam, s in summary.items():
        print(f"| {fam} | {s['pages']} | {f(s['cer'])} | {f(s['cer_excl_worst'])} | {f(s['token_recall'])} | "
              f"{f(s['transcript_line_accuracy'])} | {s['pages_per_s']} |")


if __name__ == "__main__":
    main()
