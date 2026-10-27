#!/usr/bin/env python3
"""Results rows for docs/model-lineup-eval.md.

For each engine dir under eval/out/holdout: values found (all, stamp-only dockets/dates,
the rest), per kind, transcription CER/WER (all pages, and without length-cut pages),
s/page, failed, length cuts; plus synthetic transcript4up/table CER from
eval/out/candidates/cand-<name>.json when present.

A reviewed docket/date is "stamp-only" when its text appears in the page's filing-stamp
line (Case/Doc ... Filed ... Page) in a reference reading (paddle-raw, dots-ocr-raw)
and nowhere else on that page.

usage:
    spike/.venv/bin/python eval/candidate_report.py [cand-<name> | <engine> ...]
"""
import importlib.util
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "eval"))
import score as sc  # noqa: E402

spec = importlib.util.spec_from_file_location("hs", ROOT / "eval/holdout/score.py")
hs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hs)

HOLD = ROOT / "eval/out/holdout"
TRUTH = json.load(open(ROOT / "eval/holdout/holdout_v1.json"))
STAMP = re.compile(r"(filed.*page|doc(ument)?\s*#?\s*[\d-]+.*filed)", re.I)
REFS = ["cand-paddle-raw", "cand-dots-ocr-raw"]


def flat(s):
    return re.sub(r"[^0-9a-z]", "", s.lower())


def stamp_keys():
    """(doc, page, kind, typed) of stamp-only reviewed values."""
    out = set()
    refs = [r for r in REFS if (HOLD / r).is_dir()]
    for pg in TRUTH["pages"]:
        doc, n = pg["doc"], pg["page"]
        ref = [v for v in pg["values"] + pg.get("added", [])
               if v.get("decision") == "correct" and v.get("printed") and v["kind"] in ("docket", "date")]
        for v in ref:
            votes = []
            for r in refs:
                body = hs.md_pages(HOLD / r / f"{doc}.md").get(n, "")
                lines = body.splitlines()
                st = " ".join(l for l in lines if STAMP.search(l))
                rest = " ".join(l for l in lines if not STAMP.search(l))
                p = flat(v["printed"])
                if st:
                    votes.append(p in flat(st) and p not in flat(rest))
            if votes and any(votes):
                out.add((doc, n, v["kind"], hs.typed(v["kind"], v["printed"])))
    return out


def score_engine(eng, stamps):
    d = HOLD / eng
    run = json.load(open(d / "run.json")) if (d / "run.json").exists() else {}
    cut_pages = set()
    for rj in d.glob("*.raw.json"):
        for x in json.load(open(rj)):
            if x.get("finish_reason") == "length":
                cut_pages.add((rj.name.split(".")[0], x["page"]))
    tot = {"all": [0, 0], "stamp": [0, 0], "other": [0, 0]}
    kinds = {}
    t = {k: 0 for k in ("cer", "chars", "wer", "words", "cer_nc", "chars_nc", "wer_nc", "words_nc", "pages_nc")}
    for pg in TRUTH["pages"]:
        doc, n = pg["doc"], pg["page"]
        rep = {q["page"]: q for q in json.load(open(d / f"{doc}.verify.json"))["pages"]}
        fields = [f for f in rep.get(n, {}).get("fields", []) if f["kind"] != "bates"]
        pool = {hs.typed(f["kind"], f["text"]) for f in fields}
        ref = [v for v in pg["values"] + pg.get("added", []) if v.get("decision") == "correct" and v.get("printed")]
        for k in {hs.typed(v["kind"], v["printed"]) for v in ref}:
            hit = k in pool
            grp = "stamp" if (doc, n, k[0], k) in stamps else "other"
            for g in ("all", grp):
                tot[g][0] += hit
                tot[g][1] += 1
            kk = kinds.setdefault(k[0], [0, 0])
            kk[0] += hit
            kk[1] += 1
        if pg.get("text"):
            out = hs.md_pages(d / f"{doc}.md").get(n, "")
            r = " ".join(sc.normalize_lines(pg["text"]))
            o = " ".join(sc.normalize_lines(out))
            ce, we = sc.edit_distance(r, o), sc.edit_distance(r.split(), o.split())
            t["cer"] += ce; t["chars"] += len(r); t["wer"] += we; t["words"] += len(r.split())
            if (doc, n) not in cut_pages:
                t["cer_nc"] += ce; t["chars_nc"] += len(r); t["wer_nc"] += we; t["words_nc"] += len(r.split())
                t["pages_nc"] += 1
    return run, tot, kinds, t, cut_pages


def synth(name):
    f = ROOT / "eval/out/candidates" / f"cand-{name}.json"
    if not f.exists():
        return {}
    res = json.load(open(f))
    fam = res.get("families") or res.get("summary") or {}
    out = {}
    for k in ("transcript4up", "table", "pleading", "contract", "fax"):
        v = fam.get(k)
        if isinstance(v, dict):
            out[k] = v.get("cer")
    return out


def pct(a, b):
    return f"{a / b:.1%}" if b else "n/a"


def main():
    stamps = stamp_keys()
    engines = sys.argv[1:] or sorted(p.name for p in HOLD.iterdir() if (p / "h01.verify.json").exists())
    print(f"stamp-only reviewed values: {len(stamps)} "
          f"(dockets {sum(k[2] == 'docket' for k in stamps)}, dates {sum(k[2] == 'date' for k in stamps)})\n")
    print("| Name | Found (of 292) | Stamp-only found | Other found | Amounts | Dates | Dockets | Citations "
          "| CER / word error | CER / word error w/o length cuts | s/page | Failed | Length cuts "
          "| Synth transcript4up CER | Synth table CER |")
    print("|" + "---|" * 15)
    for e in engines:
        run, tot, kinds, t, cuts = score_engine(e, stamps)
        s = synth(e.removeprefix("cand-"))
        kf = lambda k: f"{kinds.get(k, [0, 0])[0]}/{kinds.get(k, [0, 0])[1]}"
        fmt = lambda x: f"{x:.1%}" if isinstance(x, (int, float)) else "—"
        print(f"| {e} | {tot['all'][0]} ({pct(*tot['all'])}) | {tot['stamp'][0]}/{tot['stamp'][1]} "
              f"| {tot['other'][0]}/{tot['other'][1]} ({pct(*tot['other'])}) | {kf('amount')} | {kf('date')} "
              f"| {kf('docket')} | {kf('citation')} | {pct(t['cer'], t['chars'])} / {pct(t['wer'], t['words'])} "
              f"| {pct(t['cer_nc'], t['chars_nc'])} / {pct(t['wer_nc'], t['words_nc'])} ({t['pages_nc']} pages) "
              f"| {run.get('s_per_page', '—')} | {run.get('failed', '—')} | {run.get('length_cut', len(cuts))} "
              f"| {fmt(s.get('transcript4up'))} | {fmt(s.get('table'))} |")


def synth_table(names):
    fams = ("pleading", "contract", "transcript4up", "fax", "table")
    print("\nSynthetic (seeds 10-12), CER (excluding the worst page) per family; length cuts:\n")
    print("| Name | " + " | ".join(fams) + " | Length cuts | Failed | pages/s (table) |")
    print("|" + "---|" * (len(fams) + 4))
    for n in names:
        f = ROOT / "eval/out/candidates" / f"cand-{n}.json"
        if not f.exists():
            continue
        s = json.load(open(f))["summary"]
        cell = lambda k: f"{s[k]['cer']:.1%} ({s[k]['cer_excl_worst']:.1%})" if k in s else "\u2014"
        print(f"| {n} | " + " | ".join(cell(k) for k in fams)
              + f" | {sum(v.get('length_cut', 0) for v in s.values())} | {sum(v.get('failed', 0) for v in s.values())}"
              f" | {s.get('table', {}).get('pages_per_s', '\u2014')} |")


if __name__ == "__main__":
    main()
    synth_table([e.removeprefix("cand-") for e in (sys.argv[1:] or [])]
                or ["paddle-raw", "glm-ocr", "ovisocr3", "dots-ocr-raw", "dots-mocr", "lighton-4b", "olmocr-raw"])
