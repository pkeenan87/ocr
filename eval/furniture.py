#!/usr/bin/env python3
"""Count ground-truth furniture strings (headers/footers/Bates) still present in outputs.
usage: furniture.py RESULTS_DIR/TAG SET_DIR... (e.g. eval/out/results/apple-contract-furn eval/out/seed0 ...)"""
import json, sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
import score as sc
tag = Path(sys.argv[1]); kept = total = 0
for sd in sys.argv[2:]:
    sd = Path(sd); gt = json.load(open(sd / "contract.gt.json"))
    pages = sc.split_pages((tag / sd.name / "contract.md").read_text())
    for i, gp in enumerate(gt["pages"], 1):
        body = sc.normalize(pages.get(i, ("", ""))[0])
        for f in gp.get("furniture", []):
            if "Bates" in f or f == gp.get("bates"): continue
            total += 1; kept += sc.normalize(f) in body
print(f"{tag.name}: furniture strings (excluding Bates) still in page body: {kept}/{total}")
