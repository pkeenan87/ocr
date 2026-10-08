#!/usr/bin/env python3
"""Judge the public court-filing set across whatever engine outputs exist (eval/PUBLIC_SET.md).

usage: spike/.venv/bin/python -I eval/public_compare.py [--engines apple,paddleocr-vl,...] [--ids a,b]

For each document of eval/public_set.json, finds eval/out/public/<engine>/<id>.md for every
engine directory present, runs `eval/judge.py --engine-compare` on them (per-page table printed,
JSON saved to eval/out/public/judge/compare/<id>.json), then prints an aggregate table per engine
and category: flag rate per question at the calibrated thresholds, mean readability, and the
share of pages with readability < 3.5. Only pages present in every compared output count in the
aggregate, so engines are compared on the same pages. Judge responses are cached, so re-runs and
the Apple outputs judged earlier cost nothing. Public documents only (JUDGE.md privacy rule).
"""
import argparse, collections, glob, json, os, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
PUB = os.path.join(HERE, "out", "public")
SKIP_DIRS = {"pdfs", "judge", "state", "meta", "png"}
sys.path.insert(0, HERE)
from judge import THRESHOLDS, READABILITY_FLAG, BOOL_QS  # noqa: E402


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--engines", help="comma-separated engine dirs (default: all with outputs)")
    ap.add_argument("--ids", help="comma-separated document ids (default: all in the manifest)")
    a = ap.parse_args()

    docs = json.load(open(os.path.join(HERE, "public_set.json")))["documents"]
    if a.ids:
        want = set(a.ids.split(","))
        docs = [d for d in docs if d["id"] in want]
    engines = a.engines.split(",") if a.engines else sorted(
        e for e in os.listdir(PUB)
        if e not in SKIP_DIRS and os.path.isdir(os.path.join(PUB, e)) and glob.glob(os.path.join(PUB, e, "*.md")))
    if not engines:
        sys.exit("no engine outputs under " + PUB)
    print("engines:", ", ".join(engines))
    os.makedirs(os.path.join(PUB, "judge", "compare"), exist_ok=True)

    # agg[(engine, category)] -> list of per-page results
    agg = collections.defaultdict(list)
    for d in docs:
        paths = [(e, os.path.join(PUB, e, d["id"] + ".md")) for e in engines]
        paths = [(e, p) for e, p in paths if os.path.exists(p)]
        if not paths:
            print("\n## %s: no outputs" % d["id"])
            continue
        out_json = os.path.join(PUB, "judge", "compare", d["id"] + ".json")
        print("\n## %s (%s) — %s" % (d["id"], d["category"], ", ".join(e for e, _ in paths)))
        cmd = [sys.executable, "-I", os.path.join(HERE, "judge.py"), "--json", out_json]
        if len(paths) > 1:
            cmd.append("--engine-compare")
        cmd += [p for _, p in paths]
        subprocess.run(cmd, cwd=REPO, check=True)
        res = json.load(open(out_json))["docs"]
        per_engine = {e: res[p]["pages"] for e, p in paths}
        common = set.intersection(*(set(pp) for pp in per_engine.values()))
        for e, pp in per_engine.items():
            for n in common:
                agg[(e, d["category"])].append(pp[n])
                agg[(e, "ALL")].append(pp[n])

    cats = [c for c in ["complaint", "motion_brief", "deposition", "contract", "order", "scanned", "ALL"]
            if any(k[1] == c for k in agg)]
    print("\n| engine | category | pages | " + " | ".join(BOOL_QS) + " | mean readability | readability < %.1f |"
          % READABILITY_FLAG)
    print("|---|---|---|" + "---|" * len(BOOL_QS) + "---|---|")
    for c in cats:
        for e in engines:
            rows = agg.get((e, c))
            if not rows:
                continue
            cells = []
            for q in BOOL_QS:
                v = [r[q] for r in rows if r.get(q) is not None]
                cells.append("-" if not v else "%.0f%% (%d)" % (100 * sum(x >= THRESHOLDS[q] for x in v) / len(v), len(v)))
            rd = [r["readability"] for r in rows if r.get("readability") is not None]
            print("| %s | %s | %d | %s | %.2f | %.0f%% |" % (
                e, c, len(rows), " | ".join(cells), sum(rd) / len(rd),
                100 * sum(x < READABILITY_FLAG for x in rd) / len(rd)))


if __name__ == "__main__":
    main()
