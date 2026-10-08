#!/usr/bin/env python3
"""Why are transcript lines not exact? usage: anchor_diag.py RUN_DIR SETS_DIR seed [seed ...]

For every ground-truth `N text` line missing from the output, says whether the text is present
on some other numbered line (a numbering error) or not (the engine's text is wrong or missing).
"""
import json, re, sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import score  # noqa: E402

run, sets = Path(sys.argv[1]), Path(sys.argv[2])
tot = bad_num = bad_text = 0
for seed in sys.argv[3:]:
    gt = json.load(open(sets / f"seed{seed}/transcript4up.gt.json"))
    pages = score.split_pages(open(run / f"seed{seed}/transcript4up.md").read())
    for i, gp in enumerate(gt["pages"], 1):
        out = set(score.normalize_lines(pages[i][0]))
        out_text = {re.sub(r"^\d{1,2} ", "", l) for l in out}
        for l in score.normalize_lines(gp["text"]):
            if not re.match(r"\d{1,2} ", l):
                continue
            tot += 1
            if l in out:
                continue
            if re.sub(r"^\d{1,2} ", "", l) in out_text:
                bad_num += 1
                print(f"seed {seed} sheet {i} NUMBER  {l}")
            else:
                bad_text += 1
print(f"{tot} lines: {bad_num} wrong number (text present), {bad_text} text wrong or missing")
