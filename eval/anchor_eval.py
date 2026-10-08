#!/usr/bin/env python3
"""Transcript line-number measurement for the re-anchoring work (docs/spike.md).

usage: anchor_eval.py --engine apple|paddleocr-vl|dots-ocr|olmocr --seeds 0 1 2 [--sets DIR]
                      [--out DIR] [--config DIR --state DIR]

Runs `ocr scan` on <sets>/seed<N>/transcript4up.pdf and scores it with eval/score.py (imported,
unmodified): transcript line accuracy (exact "N text" lines) and the line_numbers front matter.
Never touches eval/out/state: pass an isolated --config/--state (model engines need a server
running in that state).
"""
import argparse, json, os, re, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "eval"))
import score as scorer  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--engine", required=True)
    ap.add_argument("--seeds", nargs="+", type=int, required=True)
    ap.add_argument("--sets", default=str(ROOT / "eval/out/anchor_sets"))
    ap.add_argument("--out", default=str(ROOT / "eval/out/anchor_runs"))
    ap.add_argument("--config")
    ap.add_argument("--state")
    ap.add_argument("--ocr", default=str(ROOT / "target/release/ocr"))
    ap.add_argument("--tag", default="")
    ap.add_argument("--dpi", type=int)
    a = ap.parse_args()
    env = dict(os.environ)
    if a.config:
        env["OCR_CONFIG_DIR"] = a.config
    if a.state:
        env["OCR_STATE_DIR"] = a.state
    tl_ok = tl_total = 0
    anchored = unverified = pages = 0
    for seed in a.seeds:
        d = Path(a.sets) / f"seed{seed}"
        out = Path(a.out) / (a.engine + a.tag) / f"seed{seed}"
        out.mkdir(parents=True, exist_ok=True)
        md = out / "transcript4up.md"
        r = subprocess.run([a.ocr, "scan", "--input-file", str(d / "transcript4up.pdf"),
                            "--output-file", str(md), "--model", a.engine, "--overwrite",
                            "--quiet"] + (["--dpi", str(a.dpi)] if a.dpi else []), env=env, capture_output=True, text=True)
        if r.returncode != 0:
            print(f"seed {seed}: scan failed ({r.returncode}): {r.stderr[-400:]}")
            continue
        text = md.read_text()
        gt = json.load(open(d / "transcript4up.gt.json"))
        res = scorer.score(gt, text)
        tl_ok += res["tl_ok"]
        tl_total += res["tl_total"]
        m = re.search(r"line_numbers: \{anchored: \[(.*?)\], unverified: \[(.*?)\]\}", text)
        n_a = len([x for x in m.group(1).split(",") if x.strip()]) if m else 0
        n_u = len([x for x in m.group(2).split(",") if x.strip()]) if m else 0
        anchored += n_a
        unverified += n_u
        pages += res["pages"]
        print(f"seed {seed}: lines {res['tl_ok']}/{res['tl_total']}  cer {res['cer']}  "
              f"anchored {n_a} unverified {n_u} (of {res['pages']} sheets)")
    print(f"{a.engine}: lines exact {tl_ok}/{tl_total} = "
          f"{(tl_ok / tl_total if tl_total else 0):.1%}; sheets anchored {anchored}, "
          f"unverified {unverified}")


if __name__ == "__main__":
    main()
