"""Score an engine's `--verify-json` output against the human-checked holdout set.

usage:
    PY -I eval/holdout/score.py --engine apple [--out eval/out/holdout] [--json result.json]

Reads eval/holdout/holdout_v1.json (the reviewed values and transcriptions) and, for the engine,
eval/out/holdout/<engine>/<id>.md and .verify.json, as `eval/holdout/prepare.py scan` writes them.

Values are compared by their typed value, with this file's own parser, not `ocr`'s, so the set
does not share the extractor's blind spots:
- amounts in signed cents;
- dates as ISO dates (numeric, or month names in any case);
- dockets by court office, year, type and number (judge initials left out, by the set's
  decision), citations and Bates numbers by their letters, digits and signs;
- percents by number.

The review lists each distinct value of a page once (repeats are merged), so a reviewed value is
found when any engine value of the same kind on that page has the same typed value, and an engine
value is in the set when its typed value is among the page's reviewed values.

Reported:
- **found:** reviewed values the engine extracted. The rest were dropped (missed by the
  extractor or misread).
- **verified wrong:** engine values marked `verified` that match no reviewed value on the page,
  or match a reviewed value only after a different reading. These are listed for inspection.
  The set lists the values the reviewer saw, so an engine can find a real value the review
  missed.
- **status of the values found:** verified, check or unchecked. Wrong readings (an engine value
  that is near a reviewed value but not equal to it) are listed.
- **transcription:** CER and word error, strict and normalized, on the 12 transcribed pages.
"""

import argparse
import collections
import json
import os
import re
import sys
import unicodedata

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(ROOT, "eval"))
import score as sc  # noqa: E402  (eval/score.py: normalization and edit distance)

PAGE = re.compile(r"<!-- page (\d+)[^>]*-->")
MONTHS = {m: i + 1 for i, m in enumerate(
    ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"])}


def cents(text):
    t = unicodedata.normalize("NFKC", text).strip()
    neg = t.startswith(("-", "−", "(")) or "$-" in t
    m = re.search(r"\d[\d,]*(?:\.\d+)?", t)
    if not m:
        return None
    num = m.group(0).replace(",", "")
    scale = 1
    for w, s in (("thousand", 10**3), ("million", 10**6), ("billion", 10**9)):
        if w in t.lower():
            scale = s
    whole, _, frac = num.partition(".")
    try:
        v = (int(whole) * 100 + int((frac + "00")[:2])) * scale
    except ValueError:
        return None
    return -v if neg else v


def iso_date(text):
    t = unicodedata.normalize("NFKC", text).strip().lower()
    m = re.search(r"\b(\d{1,2})/(\d{1,2})/(\d{2,4})\b", t)
    if m:
        mo, d, y = int(m.group(1)), int(m.group(2)), int(m.group(3))
    else:
        m = re.search(r"([a-z]{3})[a-z]*\.?\s+(\d{1,2})(?:st|nd|rd|th)?(?:,\s*|\s+)(\d{4})", t) or \
            re.search(r"(\d{1,2})(?:st|nd|rd|th)?\s+(?:day\s+of\s+)?([a-z]{3})[a-z]*\.?,?\s+(\d{4})", t)
        if not m:
            return None
        a, b, y = m.group(1), m.group(2), int(m.group(3))
        mon, d = (a, b) if a.isalpha() else (b, a)
        if mon[:3] not in MONTHS:
            return None
        mo, d = MONTHS[mon[:3]], int(d)
    if y < 100:
        y += 1900 if y >= 50 else 2000
    return f"{y:04d}-{mo:02d}-{d:02d}"


def chars(text):
    return re.sub(r"[^0-9a-z§¶]", "", unicodedata.normalize("NFKC", text).lower())


def chars_keep(text):
    """Lower case, letters, digits, `:` and `-` only (the separators of a docket number)."""
    return re.sub(r"[^0-9a-z:\-]", "", unicodedata.normalize("NFKC", text).lower())


def typed(kind, text):
    if kind == "amount":
        v = cents(text)
    elif kind == "date":
        v = iso_date(text)
    elif kind == "docket":
        # Court office, year, type and number; judge initials after the number are left out
        # (the set's decision: `15-CV-1376-JLS` is not wrong against `15-CV-1376-JLS-JLB`).
        m = re.match(r"(?:(\d{1,2})[:\-])?(\d{2})-?([a-z]{2,4})-?0*(\d+)", chars_keep(text))
        v = f"{m.group(1) or ''}:{m.group(2)}-{m.group(3)}-{m.group(4)}" if m else None
    elif kind == "percent":
        m = re.search(r"-?\d+(?:\.\d+)?", text)
        v = float(m.group(0)) if m else None
    else:
        v = chars(text)
    return (kind, v if v is not None else chars(text))


def md_pages(path):
    with open(path, encoding="utf-8") as f:
        md = f.read()
    body = md.split("\n---\n", 1)[1] if md.startswith("---\n") else md
    parts = PAGE.split(body)
    return {int(parts[i]): parts[i + 1] for i in range(1, len(parts) - 1, 2)}


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--engine", required=True)
    p.add_argument("--out", default=os.path.join(ROOT, "eval/out/holdout"))
    p.add_argument("--set", default=os.path.join(ROOT, "eval/holdout/holdout_v1.json"))
    p.add_argument("--json")
    a = p.parse_args()
    with open(a.set, encoding="utf-8") as f:
        truth = json.load(f)

    c = collections.Counter()
    by_kind = collections.defaultdict(collections.Counter)
    ex = collections.defaultdict(list)
    t_err = collections.Counter()
    reports, bodies = {}, {}
    for pg in truth["pages"]:
        doc, n = pg["doc"], pg["page"]
        if doc not in reports:
            with open(f"{a.out}/{a.engine}/{doc}.verify.json", encoding="utf-8") as f:
                reports[doc] = {q["page"]: q for q in json.load(f)["pages"]}
            bodies[doc] = md_pages(f"{a.out}/{a.engine}/{doc}.md")
        fields = [f for f in reports[doc].get(n, {}).get("fields", []) if f["kind"] != "bates"]
        ref = [v for v in pg["values"] + pg.get("added", []) if v.get("decision") == "correct" and v.get("printed")]
        pool = [(typed(f["kind"], f["text"]), f) for f in fields]
        keys = {typed(v["kind"], v["printed"]) for v in ref}
        for k in keys:
            c["values"] += 1
            by_kind[k[0]]["values"] += 1
            hits = [f for fk, f in pool if fk == k]
            if not hits:
                c["dropped"] += 1
                by_kind[k[0]]["dropped"] += 1
                printed = next(v["printed"] for v in ref if typed(v["kind"], v["printed"]) == k)
                near = [f["text"] for fk, f in pool if fk[0] == k[0] and fk not in keys
                        and sc.edit_distance(chars(f["text"]), chars(printed)) <= 2]
                ex["dropped" if not near else "misread"].append((doc, n, k[0], printed, near[:2]))
                continue
            # The best status any occurrence got.
            st = min((f["status"] for f in hits), key=["verified", "check", "unchecked"].index)
            c[f"found_{st}"] += 1
            by_kind[k[0]]["found"] += 1
        for fk, f in pool:
            if fk in keys:
                continue
            c["extra"] += 1
            if f["status"] == "verified":
                c["extra_verified"] += 1
                ex["verified, not in the set"].append((doc, n, f["kind"], f["text"], f["why"]))
        if pg.get("text"):
            out = bodies[doc].get(n, "")
            for mode in ("", "_strict"):
                strict = mode == "_strict"
                r = " ".join(sc.normalize_lines(pg["text"], strict=strict))
                o = " ".join(sc.normalize_lines(out, strict=strict))
                t_err["cer" + mode] += sc.edit_distance(r, o)
                t_err["chars" + mode] += len(r)
                t_err["wer" + mode] += sc.edit_distance(r.split(), o.split())
                t_err["words" + mode] += len(r.split())
            t_err["pages"] += 1

    found = sum(v for k, v in c.items() if k.startswith("found_"))
    pct = lambda x, y: f"{x / y:.1%}" if y else "n/a"
    print(f"# holdout v1: {a.engine}\n")
    print(f"- reviewed values {c['values']}: found {found} ({pct(found, c['values'])}), "
          f"dropped or misread {c['dropped']}")
    print(f"- of those found: verified {c['found_verified']}, check {c['found_check']}, "
          f"unchecked {c['found_unchecked']}")
    print(f"- engine values not in the set: {c['extra']}, of which verified {c['extra_verified']} "
          f"(inspect: a misread, or a real value the review did not list)")
    for k, v in sorted(by_kind.items()):
        print(f"    {k:9} {v['found']}/{v['values']} found")
    if t_err["pages"]:
        print(f"- transcription ({t_err['pages']} pages): CER {pct(t_err['cer'], t_err['chars'])}, "
              f"strict {pct(t_err['cer_strict'], t_err['chars_strict'])}; word error "
              f"{pct(t_err['wer'], t_err['words'])}, strict {pct(t_err['wer_strict'], t_err['words_strict'])}")
    for k, e in ex.items():
        print(f"\n{k} ({len(e)}):")
        for x in e[:40]:
            print("   ", x)
    if a.json:
        with open(a.json, "w", encoding="utf-8") as f:
            json.dump({"engine": a.engine, "counts": dict(c), "by_kind": {k: dict(v) for k, v in by_kind.items()},
                       "transcription": dict(t_err), "examples": {k: e for k, e in ex.items()}}, f, indent=1)


if __name__ == "__main__":
    main()
