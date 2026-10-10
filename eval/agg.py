#!/usr/bin/env python3
"""Aggregate run.py result JSONs over chosen families: agg.py FILE.json [--families a,b] [--exclude x]"""
import json, sys, argparse
ap = argparse.ArgumentParser(); ap.add_argument("files", nargs="+"); ap.add_argument("--families"); ap.add_argument("--exclude", default="transcript4up,production")
a = ap.parse_args()
for fn in a.files:
    r = json.load(open(fn)); fams = a.families.split(",") if a.families else [f for f in r["runs"] if f not in a.exclude.split(",")]
    d = lambda k: sum(e[k] for f in fams for e in r["runs"][f].values() if "error" not in e)
    # Throughput over the runs that produced a result, so a fast failure does not inflate it.
    pg, w = d("pages"), d("wall_s")
    failed = sum("error" in e for f in fams for e in r["runs"][f].values())
    print(f"{r['tag']}: fams={fams} CER={d('dist_sum')/d('len_sum'):.4f} tokrecall={d('tokens_found')/d('tokens_total'):.4f} "
          f"bates={d('bates_ok')}/{d('bates_total')} pages={pg} pages/s={pg/w:.2f} failed_runs={failed}")
