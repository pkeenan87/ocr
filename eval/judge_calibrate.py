#!/usr/bin/env python3
"""Calibrate eval/judge.py (Jev) against ground truth. Dev only; synthetic fixtures only.

usage: judge_calibrate.py [--seeds 10-19] [--results eval/out/results/apple-accept-dpi200]
                          [--dry-run] [--workers 8] [--out eval/out/judge]

1. clean control: ground-truth pages rendered as Markdown (plus furniture; pleading also with
   margin line numbers 1-28),
2. injected defects with known labels: interleave, corrupt 5/15/30%, repetition loop, broken table,
3. real Apple outputs, per-page CER from score.py (imported, unchanged),
then reports AUC / precision / recall / FPR per question and Spearman(readability, CER).
Fixtures and labels are written to <out>/injected/, results to <out>/calibration.json and report.md.
"""
import argparse, glob, json, os, random, re, sys, textwrap
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import judge  # noqa: E402
import score as scorer  # noqa: E402

FAMILIES = ["pleading", "contract", "transcript4up", "fax", "table"]
QS = judge.BOOL_QS


# ---------------------------------------------------------------- rendering ground truth
def is_table_line(ln):
    return ln.count(" | ") >= 2 or (ln.count("|") >= 2 and ln.strip().startswith(("TOTAL", "INV")))


def render_md(text):
    """GT plain text -> Markdown; runs of pipe rows become a GFM table with a separator row."""
    lines = text.split("\n")
    out, i = [], 0
    while i < len(lines):
        if is_table_line(lines[i]):
            j = i
            while j < len(lines) and is_table_line(lines[j]):
                j += 1
            rows = [[c.strip() for c in ln.split("|")] for ln in lines[i:j]]
            n = max(len(r) for r in rows)
            rows = [r + [""] * (n - len(r)) for r in rows]
            if out and out[-1] != "":
                out.append("")
            out.append("| " + " | ".join(rows[0]) + " |")
            out.append("|" + "---|" * n)
            out += ["| " + " | ".join(r) + " |" for r in rows[1:]]
            out.append("")
            i = j
        else:
            out.append(lines[i])
            i += 1
    return "\n".join(out).strip() + "\n"


def with_furniture(md, furniture):
    return md + ("\n" + "\n\n".join(furniture) + "\n" if furniture else "")


def wrap_lines(text, width):
    out = []
    for para in text.split("\n"):
        if not para.strip():
            continue
        out += textwrap.wrap(para, width) if not para.startswith("|") else [para]
    return out


def pleading_lined(text, furniture):
    """Pleading-paper style: wrapped lines with margin numbers 1..28 restarting each 28 lines."""
    lines = wrap_lines(text, 70)
    out = ["%d %s" % (i % 28 + 1, ln) for i, ln in enumerate(lines)]
    return "\n".join(out) + "\n\n" + "\n\n".join(furniture) + "\n"


# ---------------------------------------------------------------- injections
def inject_interleave(text, rng):
    """Simulated column merge: wrap at column width, interleave the two halves line by line."""
    lines = wrap_lines(text, 45)
    h = len(lines) // 2
    a, b = lines[:h], lines[h:]
    out = []
    for i in range(max(len(a), len(b))):
        if i < len(a):
            out.append(a[i])
        if i < len(b):
            out.append(b[i])
    return "\n".join(out) + "\n"


CONFUSIONS = [("rn", "m"), ("m", "rn"), ("l", "1"), ("1", "l"), ("O", "0"), ("0", "O"),
              ("e", "c"), ("cl", "d"), ("d", "cl"), ("i", "l"), ("h", "b"), ("u", "n"), ("S", "5"),
              ("a", "o"), ("I", "l")]


def corrupt_word(w, rng):
    ops = []
    for a, b in CONFUSIONS:
        if a in w:
            ops.append(("sub", a, b))
    if len(w) > 2 and w[0].isalpha():
        ops.append(("drop",))
    if not ops:
        ops.append(("trunc",))
    op = rng.choice(ops)
    if op[0] == "sub":
        idx = [m.start() for m in re.finditer(re.escape(op[1]), w)]
        k = rng.choice(idx)
        return w[:k] + op[2] + w[k + len(op[1]):]
    if op[0] == "drop":
        return w[1:]
    return w[:max(1, len(w) - 2)]


def inject_corrupt(md, rate, rng):
    """Corrupt ~rate of words with OCR confusions, dropped first letters, merges, truncation."""
    toks = re.split(r"(\s+)", md)
    widx = [i for i, t in enumerate(toks) if re.search(r"[A-Za-z]{2,}", t) and "|" not in t
            and not t.startswith(("#", "---"))]
    n = max(1, round(len(widx) * rate))
    chosen = sorted(rng.sample(widx, min(n, len(widx))))
    for i in chosen:
        if rng.random() < 0.15 and i + 1 < len(toks) and toks[i + 1] == " ":
            toks[i + 1] = ""  # merged words
        else:
            toks[i] = corrupt_word(toks[i], rng)
    return "".join(toks)


def inject_repetition(md, rng):
    words = re.sub(r"[|#*]", " ", md).split()
    for _ in range(50):
        start = rng.randrange(max(1, len(words) - 15))
        phrase = ""
        for w in words[start:]:
            if len(phrase) + len(w) + 1 > 80:
                break
            phrase = (phrase + " " + w).strip()
        if 40 <= len(phrase) <= 80:
            break
    sep = rng.choice([" ", "\n"])
    return md.rstrip() + "\n\n" + sep.join([phrase] * 20) + "\n"


def inject_table(md, mode, rng):
    lines = md.split("\n")
    rows = [i for i, ln in enumerate(lines) if ln.startswith("|") and not ln.startswith("|---")]
    body = rows[1:]
    if mode == "shuffle":  # cells shuffled within ~60% of body rows
        for i in body:
            if rng.random() < 0.6:
                cells = lines[i].strip("|").split("|")
                rng.shuffle(cells)
                lines[i] = "|" + "|".join(cells) + "|"
    elif mode == "drop_pipes":  # pipes dropped from ~60% of rows, cells run together
        for i in rows:
            if rng.random() < 0.6:
                cells = [c.strip() for c in lines[i].strip("|").split("|")]
                keep = rng.sample(range(1, len(cells)), max(1, (len(cells) - 1) // 2))
                s = cells[0]
                for k in range(1, len(cells)):
                    s += (" | " if k in keep else " ") + cells[k]
                lines[i] = "| " + s + " |"
    elif mode == "merge":  # pairs of rows merged into one line
        for i in body[::3]:
            if i + 1 < len(lines) and lines[i + 1].startswith("|"):
                lines[i] = lines[i].rstrip("|").rstrip() + " " + lines[i + 1].lstrip("|").strip()
                lines[i + 1] = None
        lines = [ln for ln in lines if ln is not None]
    return "\n".join(lines)


# ---------------------------------------------------------------- stats
def auc(pos, neg):
    """Mann-Whitney AUC: P(score(pos) > score(neg)), ties 0.5."""
    if not pos or not neg:
        return None
    s = 0.0
    for p in pos:
        for n in neg:
            s += 1.0 if p > n else 0.5 if p == n else 0.0
    return round(s / (len(pos) * len(neg)), 3)


def ranks(xs):
    order = sorted(range(len(xs)), key=lambda i: xs[i])
    r = [0.0] * len(xs)
    i = 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and xs[order[j + 1]] == xs[order[i]]:
            j += 1
        for k in range(i, j + 1):
            r[order[k]] = (i + j) / 2 + 1
        i = j + 1
    return r


def spearman(x, y):
    if len(x) < 3:
        return None
    rx, ry = ranks(x), ranks(y)
    mx, my = sum(rx) / len(rx), sum(ry) / len(ry)
    num = sum((a - mx) * (b - my) for a, b in zip(rx, ry))
    den = (sum((a - mx) ** 2 for a in rx) * sum((b - my) ** 2 for b in ry)) ** 0.5
    return round(num / den, 3) if den else None


def prf(pos, neg, t):
    tp = sum(p >= t for p in pos)
    fp = sum(n >= t for n in neg)
    rec = tp / len(pos) if pos else None
    prec = tp / (tp + fp) if tp + fp else None
    fpr = fp / len(neg) if neg else None
    r3 = lambda v: None if v is None else round(v, 3)
    return {"threshold": t, "precision": r3(prec), "recall": r3(rec), "fpr": r3(fpr)}


def best_threshold(pos, neg, max_fpr=0.02):
    """Threshold on a 0.05 grid maximising Youden's J (recall - FPR) subject to clean FPR <= max_fpr;
    among ties take the median, so a perfectly separated question gets a threshold in the middle of
    the gap, not at its edge."""
    grid = [x / 20 for x in range(1, 20)]
    grid = [t for t in grid if sum(n >= t for n in neg) / len(neg) <= max_fpr] or [0.95]
    js = [(sum(p >= t for p in pos) / len(pos) - sum(n >= t for n in neg) / len(neg), t) for t in grid]
    best = max(j for j, _ in js)
    ties = [t for j, t in js if j >= best - 1e-9]
    return ties[len(ties) // 2]


# ---------------------------------------------------------------- main
def parse_seeds(s):
    a, b = s.split("-") if "-" in s else (s, s)
    return list(range(int(a), int(b) + 1))


def build_items(seeds, results, outdir, rng):
    items = []  # dicts: id, family, seed, page, variant, text, [cer]
    for seed in seeds:
        for fam in FAMILIES:
            gtp = os.path.join(HERE, "out", "seed%d" % seed, fam + ".gt.json")
            if not os.path.exists(gtp):
                continue
            gt = json.load(open(gtp))
            for pn, gp in enumerate(gt["pages"], 1):
                base = dict(family=fam, seed=seed, page=pn)
                clean = render_md(gp["text"])
                cleanf = with_furniture(clean, gp.get("furniture", []))
                items.append(dict(base, variant="clean", text=cleanf))
                if fam == "pleading":
                    items.append(dict(base, variant="clean_lined",
                                      text=pleading_lined(gp["text"], gp.get("furniture", []))))
                items.append(dict(base, variant="interleave", text=inject_interleave(clean, rng)))
                for rate in (0.05, 0.15, 0.30):
                    items.append(dict(base, variant="corrupt%02d" % round(rate * 100),
                                      text=inject_corrupt(cleanf, rate, rng)))
                items.append(dict(base, variant="repetition", text=inject_repetition(cleanf, rng)))
                if clean.count("\n|") >= 3:
                    for mode in ("shuffle", "drop_pipes", "merge"):
                        items.append(dict(base, variant="table_" + mode,
                                          text=inject_table(cleanf, mode, rng)))
            # real output
            mdp = os.path.join(results, "seed%d" % seed, fam + ".md")
            if os.path.exists(mdp):
                md = open(mdp).read()
                sc = scorer.score(gt, md, strip_line_numbers=(fam == "pleading"))
                pages = scorer.split_pages(md)
                for pn in range(1, len(gt["pages"]) + 1):
                    body = pages.get(pn, ("", ""))[0]
                    items.append(dict(family=fam, seed=seed, page=pn, variant="real", text=body,
                                      cer=sc["cer_pages"][pn - 1]))
    for i, it in enumerate(items):
        it["id"] = "s%d-%s-p%d-%s" % (it["seed"], it["family"], it["page"], it["variant"])
    inj = os.path.join(outdir, "injected")
    os.makedirs(inj, exist_ok=True)
    with open(os.path.join(inj, "labels.jsonl"), "w") as f:
        for it in items:
            if it["variant"] != "real":
                with open(os.path.join(inj, it["id"] + ".md"), "w") as g:
                    g.write(it["text"])
            f.write(json.dumps({k: v for k, v in it.items() if k != "text"}) + "\n")
    return items


def analyse(items):
    by = {}
    for it in items:
        by.setdefault(it["variant"], []).append(it)
    clean = by.get("clean", []) + by.get("clean_lined", [])
    rep = {"counts": {v: len(xs) for v, xs in by.items()}}

    # mean answer per variant x question (cross-firing matrix)
    mat = {}
    for v, xs in by.items():
        row = {}
        for q in QS + ["readability"]:
            vals = [x["j"][q] for x in xs if x["j"].get(q) is not None]
            row[q] = round(sum(vals) / len(vals), 3) if vals else None
        mat[v] = row
    rep["mean_by_variant"] = mat

    target = {"garbled": ["corrupt05", "corrupt15", "corrupt30"], "interleaved": ["interleave"],
              "repetition": ["repetition"],
              "table_broken": ["table_shuffle", "table_drop_pipes", "table_merge"]}
    perq = {}
    for q, vs in target.items():
        negs_all = [x for x in clean if x["j"].get(q) is not None]
        if q == "table_broken":  # negatives: clean pages that have a table
            negs_all = [x for x in by.get("clean", []) if x["j"].get(q) is not None]
        neg = [x["j"][q] for x in negs_all]
        res = {"negatives": len(neg)}
        allpos = []
        for v in vs:
            pos = [x["j"][q] for x in by.get(v, []) if x["j"].get(q) is not None]
            allpos += pos
            res[v] = {"n": len(pos), "auc": auc(pos, neg), "at_0.5": prf(pos, neg, 0.5)}
        t = best_threshold(allpos, neg)
        res["all"] = {"n": len(allpos), "auc": auc(allpos, neg), "at_0.5": prf(allpos, neg, 0.5),
                      "recommended": prf(allpos, neg, t)}
        for v in vs:
            pos = [x["j"][q] for x in by.get(v, []) if x["j"].get(q) is not None]
            res[v]["recommended"] = prf(pos, neg, t)
        perq[q] = res
    # readability as a detector of any injected defect (lower = worse)
    rneg = [-x["j"]["readability"] for x in clean if x["j"].get("readability") is not None]
    rr = {}
    for v in by:
        if v in ("clean", "clean_lined", "real"):
            continue
        pos = [-x["j"]["readability"] for x in by[v] if x["j"].get("readability") is not None]
        rr[v] = auc(pos, rneg)
    perq["readability_auc_vs_clean"] = rr
    rep["injected"] = perq

    # false positive rates on clean text per family at 0.5 and recommended thresholds
    thr = {q: perq[q]["all"]["recommended"]["threshold"] for q in target}
    fpr = {}
    for label, xs in [("clean:" + f, [x for x in by.get("clean", []) if x["family"] == f]) for f in FAMILIES] + \
                     [("clean_lined:pleading", by.get("clean_lined", []))]:
        row = {"n": len(xs)}
        for q in QS:
            vals = [x["j"][q] for x in xs if x["j"].get(q) is not None]
            if vals:
                row[q] = {"fpr@0.5": round(sum(v >= 0.5 for v in vals) / len(vals), 3),
                          "fpr@rec": round(sum(v >= thr[q] for v in vals) / len(vals), 3)}
        reads = [x["j"]["readability"] for x in xs if x["j"].get("readability") is not None]
        row["mean_readability"] = round(sum(reads) / len(reads), 3) if reads else None
        fpr[label] = row
    rep["clean_fpr"] = fpr
    rcut = {}
    defects = [x for v, xs in by.items() if v not in ("clean", "clean_lined", "real") for x in xs]
    real_ = by.get("real", [])
    for t in (3.0, 3.5, 4.0, 4.25, 4.5):
        rcut["read<%.2f" % t] = {
            "clean_fpr": round(sum(x["j"]["readability"] < t for x in clean) / len(clean), 3),
            "injected_recall": round(sum(x["j"]["readability"] < t for x in defects) / len(defects), 3),
            "real_flag_rate_cer<=2%": round(sum(x["j"]["readability"] < t for x in real_ if x["cer"] <= 0.02)
                                            / max(1, sum(x["cer"] <= 0.02 for x in real_)), 3),
            "real_flag_rate_cer>5%": round(sum(x["j"]["readability"] < t for x in real_ if x["cer"] > 0.05)
                                           / max(1, sum(x["cer"] > 0.05 for x in real_)), 3)}
    rep["readability_cutoffs"] = rcut
    rep["recommended_thresholds"] = thr

    # real outputs vs CER
    real = [x for x in by.get("real", []) if x["j"].get("readability") is not None]
    def corr(xs, q):
        return spearman([x["j"][q] for x in xs], [x["cer"] for x in xs])
    rl = {"n": len(real)}
    sub = {"all": real, "excl_transcript4up": [x for x in real if x["family"] != "transcript4up"]}
    for f in FAMILIES:
        sub[f] = [x for x in real if x["family"] == f]
    for name, xs in sub.items():
        rl[name] = {"n": len(xs), "spearman_readability_cer": corr(xs, "readability"),
                    "spearman_garbled_cer": corr(xs, "garbled"),
                    "spearman_interleaved_cer": corr(xs, "interleaved"),
                    "mean_cer": round(sum(x["cer"] for x in xs) / len(xs), 4) if xs else None,
                    "mean_readability": round(sum(x["j"]["readability"] for x in xs) / len(xs), 3) if xs else None}
    # bad-page detection: CER > 10% as positives
    for cut in (0.05, 0.10, 0.25):
        pos = [x for x in real if x["cer"] > cut]
        neg = [x for x in real if x["cer"] <= cut]
        rl["bad_cer>%.2f" % cut] = {
            "positives": len(pos), "negatives": len(neg),
            "auc_low_readability": auc([-x["j"]["readability"] for x in pos], [-x["j"]["readability"] for x in neg]),
            "auc_garbled": auc([x["j"]["garbled"] for x in pos], [x["j"]["garbled"] for x in neg]),
            "auc_interleaved": auc([x["j"]["interleaved"] for x in pos], [x["j"]["interleaved"] for x in neg]),
            "auc_max_defect": auc([max(x["j"][q] or 0 for q in QS) for x in pos],
                                  [max(x["j"][q] or 0 for q in QS) for x in neg]),
        }
        pe = [x for x in pos if x["family"] != "transcript4up"]
        ne = [x for x in neg if x["family"] != "transcript4up"]
        rl["bad_cer>%.2f" % cut]["excl_transcript4up"] = {
            "positives": len(pe), "negatives": len(ne),
            "auc_low_readability": auc([-x["j"]["readability"] for x in pe], [-x["j"]["readability"] for x in ne]),
            "auc_garbled": auc([x["j"]["garbled"] for x in pe], [x["j"]["garbled"] for x in ne])}
    # flag rates on real pages per family at recommended thresholds
    fr = {}
    for f in FAMILIES:
        xs = sub[f]
        fr[f] = {q: round(sum((x["j"][q] or 0) >= thr[q] for x in xs if x["j"][q] is not None)
                          / max(1, sum(x["j"][q] is not None for x in xs)), 3) for q in QS}
    rl["flag_rate_at_recommended"] = fr
    rep["real"] = rl
    return rep


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seeds", default="10-19")
    ap.add_argument("--results", default=os.path.join(HERE, "out", "results", "apple-accept-dpi200"))
    ap.add_argument("--out", default=os.path.join(HERE, "out", "judge"))
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--rng-seed", type=int, default=1234)
    a = ap.parse_args()
    rng = random.Random(a.rng_seed)
    items = build_items(parse_seeds(a.seeds), a.results, a.out, rng)
    est = sum(len(judge.chunks(x["text"])) for x in items if x["text"].strip())
    jev = judge.Jev()
    print("items %d, planned requests <= %d (cache has %d entries)" % (len(items), est, len(jev.cache)),
          file=sys.stderr)
    if est > judge.MAX_REQUESTS:
        sys.exit("planned requests exceed cap %d; narrow --seeds" % judge.MAX_REQUESTS)
    if a.dry_run:
        return
    done = [0]

    def run(it):
        try:
            it["j"] = judge.judge_text(jev, it["text"])
        except RuntimeError as e:
            it["j"] = {"error": str(e)}
        done[0] += 1
        if done[0] % 50 == 0:
            print("  %d/%d  %s" % (done[0], len(items), jev.summary()), file=sys.stderr, flush=True)
        return it

    with ThreadPoolExecutor(a.workers) as ex:
        list(ex.map(run, items))
    errs = [x for x in items if "error" in x["j"]]
    for x in errs[:5]:
        print("error", x["id"], x["j"]["error"], file=sys.stderr)
    good = [x for x in items if "error" not in x["j"] and not x["j"].get("empty")]
    rep = analyse(good)
    rep["errors"] = len(errs)
    rep["usage"] = jev.summary()
    with open(os.path.join(a.out, "calibration.json"), "w") as f:
        json.dump(rep, f, indent=1)
    with open(os.path.join(a.out, "items.jsonl"), "w") as f:
        for x in items:
            f.write(json.dumps({k: v for k, v in x.items() if k != "text"}) + "\n")
    print(json.dumps(rep, indent=1))
    print("usage:", jev.summary(), file=sys.stderr)


if __name__ == "__main__":
    main()
