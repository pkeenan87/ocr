"""Score held-out layouts against heldout.py ground truth.

usage: score_heldout.py <dir-with-pdfs-and-expected-json-and-md-outputs>
Reads <name>.expected.json and <name>.md / <name>_scan.md for every family and prints per-page
found/total, in-order pairs, and aggregate totals per variant and overall.
"""
import glob, json, os, sys

d = sys.argv[1]
totals = {}
for path in sorted(glob.glob(f"{d}/*.expected.json")):
    name = os.path.basename(path)[: -len(".expected.json")]
    seq = json.load(open(path))
    for variant in (name, f"{name}_scan"):
        md_path = f"{d}/{variant}.md"
        if not os.path.exists(md_path):
            continue
        md = open(md_path).read()
        # Skip the front matter so a prefix cannot match metadata.
        body = md.split("\n---\n", 1)[-1] if md.startswith("---\n") else md
        found = [p for p in (body.find(s) for s in seq) if p >= 0]
        inorder = sum(1 for a, b in zip(found, found[1:]) if b > a)
        pairs = max(len(found) - 1, 0)
        print(f"{variant:28s} found {len(found):3d}/{len(seq):3d}  in-order pairs {inorder:3d}/{pairs:3d}")
        key = ("scan" if variant.endswith("_scan") else "clean", name)
        t = totals.setdefault(key, [0, 0, 0, 0])
        t[0] += len(found); t[1] += len(seq); t[2] += inorder; t[3] += pairs

if totals:
    print()
    print(f"{'family/variant':28s} {'found':>11s} {'in-order pairs':>16s}")

    def line(label, keys):
        f = sum(totals[k][0] for k in keys); n = sum(totals[k][1] for k in keys)
        i = sum(totals[k][2] for k in keys); p = sum(totals[k][3] for k in keys)
        print(f"{label:28s} {f:4d}/{n:<4d} {100 * f / max(n, 1):5.1f}% {i:5d}/{p:<5d} {100 * i / max(p, 1):5.1f}%")

    for k in sorted(totals, key=lambda k: (k[1], k[0])):
        line(f"{k[1]} [{k[0]}]", [k])
    for v in ("clean", "scan"):
        line(f"ALL [{v}]", [k for k in totals if k[0] == v])
    line("ALL", list(totals))
