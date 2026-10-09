#!/usr/bin/env python3
"""Score saved/replayed transcript4up Markdown against its ground truth with eval/score.py.

usage: anchor_score.py GT.json OUT.md [OUT.md ...]   (GT is matched to each OUT by position:
       pass pairs as GT OUT GT OUT ...)
"""
import json, sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import score as scorer  # noqa: E402

ok = total = 0
args = sys.argv[1:]
for gt_path, md_path in zip(args[0::2], args[1::2]):
    r = scorer.score(json.load(open(gt_path)), open(md_path).read())
    ok += r["tl_ok"]
    total += r["tl_total"]
    print(f"{md_path}: {r['tl_ok']}/{r['tl_total']}")
print(f"total {ok}/{total} = {ok / max(total, 1):.1%}")
