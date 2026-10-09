"""Residual risk of the verification workflow: `ocr --verify-json` plus Azure on the pages it flags.

usage:
    PY -I eval/azure_verify_eval.py --bundle eval/verify_bundle/apple-all
        [--azure eval/out/public/azure-di] [--pdfs eval/out/public/pdfs] [--set eval/public_set.json]
        [--route report|all] [--json out.json]

Needs only Python (pypdfium2), so it runs off the Mac. Inputs:
- `--bundle`: a Mac run of `eval/verify_eval.py`, copied from `eval/out/verify/<run>/` to
  `eval/verify_bundle/<run>/`: `blind/<id>.verify.json` and `blind/<id>.md` (the embedded text
  hidden from verify, so the readers decide alone), and `truth/<id>.verify.json` (values graded
  by the embedded text).
- `--azure`: the raw Azure Document Intelligence responses, `<id>.azure.json` (prebuilt-layout,
  api-version 2024-11-30), covering every page in the set.
- `--pdfs`: the public PDFs, whose embedded text grades the sentences.

The workflow it simulates is the one `skills/ocr/SKILL.md` describes:
- a `verified` value is relied on as is;
- on a page Azure reads (`--route report`: the report's `azure_pages`; `--route all`: every
  page), a value Azure also has is `confirmed` and relied on, one it lacks is `disputed`;
- any other value goes to a person.
Only pages with an exact embedded text (no invisible OCR layer) are graded. A value is right when
the truth run found it in the embedded text, wrong when the truth run did not.

"Azure has the value": the value's letters and digits appear in order in Azure's lines for that
page, with at most 3 other characters between two of them and not inside a longer word or number
(`$5,000.00` matches `$ 5,000.00.`; a docket cut short does not match the full one; `§` and `$`
are ignored). A value that is also printed
elsewhere on the page can be confirmed by coincidence, so read every "confirmed but wrong" case.

Sentences: every sentence of at least 6 words on a graded page of the OCR. It is confirmed when
Azure has it word for word, right when the embedded text has it word for word, in both cases
letters and digits only, with pleading line numbers (any 1-2 digit token) skipped. So a sentence
wrong only in a 1-2 digit number counts as right. Reading order differs between the three
(headers, footnotes, columns), so read the examples.
"""

import argparse
import collections
import json
import os
import re
import unicodedata

import pypdfium2 as pdfium

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PAGE = re.compile(r"<!-- page (\d+)[^>]*-->")
SENTENCE_WORDS = 6


def tokens(text):
    """Lower-case letter and digit runs; a hyphen at a line end joins the word."""
    text = unicodedata.normalize("NFKC", text)
    text = re.sub(r"[-\u00ad\u2010\u2011]\s*\n\s*", "", text)
    return re.findall(r"[0-9a-z§¶]+", text.lower())


def value_text(text):
    """Lower case, NFKC; a line-end hyphen joins the word and keeps the hyphen."""
    text = unicodedata.normalize("NFKC", text).lower()
    text = re.sub(r"\u00ad|\ufffe", "", text)
    return re.sub(r"([-\u2010\u2011])\s*\n\s*", r"\1", text)


def has_value(hay, kind, text):
    """Whether `hay` (from `value_text`) has the value: its letters and digits in order, at most
    3 other characters between two of them, not inside a longer word or number, and for a docket
    or Bates number not cut short (`...-JGB` does not match `...-JGB-SP`)."""
    chars = re.findall(r"[0-9a-z]", value_text(text))
    if not chars:
        return False
    end = r"(?![-_.:/]?[0-9a-z])" if kind in ("docket", "bates") else r"(?![0-9a-z])(?![.,]\d)"
    pat = r"(?<![0-9a-z])" + r"[^0-9a-z]{0,3}".join(map(re.escape, chars)) + end
    return re.search(pat, hay) is not None


def joined(toks):
    return f" {' '.join(toks)} "


def sentence_pattern(toks):
    """Tokens in order, allowing pleading line numbers (1-2 digit tokens) between them on either
    side. A sentence that differs only in a 1-2 digit number therefore still matches."""
    parts = [f"(?:{t} )?" if re.fullmatch(r"\d{1,2}", t) else f"{re.escape(t)} " for t in toks]
    return re.compile(" " + "(?:\\d{1,2} )*".join(parts))


def page_set(spec, n_pages):
    if spec == "all":
        return set(range(1, n_pages + 1))
    out = set()
    for part in filter(None, (spec or "").replace(" ", "").split(",")):
        a, _, b = part.partition("-")
        out.update(range(int(a), int(b or a) + 1))
    return out


def azure_pages(path):
    with open(path, encoding="utf-8") as f:
        r = json.load(f)
    r = r.get("analyzeResult", r)
    return {p["pageNumber"]: "\n".join(line["content"] for line in p.get("lines", []))
            for p in r["pages"]}


def md_pages(path):
    with open(path, encoding="utf-8") as f:
        md = f.read()
    body = md.split("\n---\n", 1)[1] if md.startswith("---\n") else md
    parts = PAGE.split(body)
    return {int(parts[i]): parts[i + 1] for i in range(1, len(parts) - 1, 2)}


def sentences(md):
    """Sentences of a page's Markdown, without pleading line numbers or table rules."""
    text = re.sub(r"<[^>]+>|\|[-: |]+\||[|#*_`>]", " ", md)
    text = re.sub(r"(?m)^\s*\d{1,2}\s+", "", text)
    text = re.sub(r"[-\u00ad\u2010\u2011]\s*\n\s*", "", text)
    for line in text.split("\n\n"):
        for s in re.split(r"(?<=[.!?;:])\s+(?=[A-Z(\"'“])", " ".join(line.split())):
            if len(tokens(s)) >= SENTENCE_WORDS:
                yield s


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--bundle", required=True)
    p.add_argument("--azure", default=os.path.join(ROOT, "eval/out/public/azure-di"))
    p.add_argument("--pdfs", default=os.path.join(ROOT, "eval/out/public/pdfs"))
    p.add_argument("--set", default=os.path.join(ROOT, "eval/public_set.json"))
    p.add_argument("--route", choices=["report", "all"], default="report")
    p.add_argument("--json")
    a = p.parse_args()
    with open(a.set, encoding="utf-8") as f:
        docs = json.load(f)["documents"]

    c = collections.Counter()
    ex = collections.defaultdict(list)
    for d in docs:
        doc = d["id"]

        def load(kind, ext=".verify.json"):
            with open(os.path.join(a.bundle, kind, doc + ext), encoding="utf-8") as f:
                return json.load(f) if ext.endswith("json") else f.read()

        blind, truth = load("blind"), load("truth")
        az = azure_pages(os.path.join(a.azure, doc + ".azure.json"))
        md = md_pages(os.path.join(a.bundle, "blind", doc + ".md"))
        pdf = pdfium.PdfDocument(os.path.join(a.pdfs, doc + ".pdf"))
        routed = page_set("all" if a.route == "all" else blind["azure_pages"], len(blind["pages"]))
        c["pages"] += len(blind["pages"])
        c["azure_pages"] += len(routed)
        tp = {q["page"]: q for q in truth["pages"]}
        for pb in blind["pages"]:
            n = pb["page"]
            pt = tp.get(n)
            if not pt or not pt["embedded_text"] or pt.get("ocr_text_layer"):
                continue
            c["graded_pages"] += 1
            if n not in az:
                c["pages_missing_from_azure"] += 1
                ex["page missing from the Azure response"].append((doc, n))
                continue
            az_v = value_text(az[n])
            why_t = {(f["kind"], f["text"], f["line"]): f["why"] for f in pt["fields"]}
            for f in pb["fields"]:
                if f["kind"] == "bates":
                    continue
                w = why_t.get((f["kind"], f["text"], f["line"]))
                if w is None or not ({"matches_embedded_text", "not_in_embedded_text"} & set(w)):
                    c["values_ungraded"] += 1
                    continue
                right = "matches_embedded_text" in w
                has = has_value(az_v, f["kind"], f["text"])
                # Azure alone, on every graded value: how often it repeats a wrong reading.
                c[f"azure_has_{'right' if right else 'wrong'}"] += has
                c["right" if right else "wrong"] += 1
                if not right and has:
                    ex["Azure repeats a wrong value"].append((doc, n, f["kind"], f["text"]))
                if right and not has:
                    ex["Azure lacks a right value"].append((doc, n, f["kind"], f["text"]))
                if f["status"] == "verified":
                    outcome = "verified"
                elif n in routed:
                    outcome = "confirmed" if has else "disputed"
                else:
                    outcome = "person"
                c[f"{outcome}_{'right' if right else 'wrong'}"] += 1
                if not right and outcome in ("verified", "confirmed"):
                    ex[f"{outcome} but wrong"].append((doc, n, f["kind"], f["text"]))
            if n not in routed:
                continue
            emb_t = joined(tokens(pdf[n - 1].get_textpage().get_text_range()))
            az_t = joined(tokens(az[n]))
            for s in sentences(md.get(n, "")):
                pat = sentence_pattern(tokens(s))
                right, has = bool(pat.search(emb_t)), bool(pat.search(az_t))
                c["sentences"] += 1
                c[f"sentences_{'confirmed' if has else 'unconfirmed'}_"
                  f"{'right' if right else 'unmatched'}"] += 1
                if has and not right:
                    ex["sentence Azure confirms, not in embedded text"].append((doc, n, s[:90]))

    def pct(x, y):
        return f"{x / y:.1%}" if y else "n/a"

    relied = sum(c[f"{o}_{r}"] for o in ("verified", "confirmed") for r in ("right", "wrong"))
    relied_wrong = c["verified_wrong"] + c["confirmed_wrong"]
    print(f"# ocr + Azure: {os.path.basename(os.path.normpath(a.bundle))}, route {a.route}\n")
    print(f"- pages {c['pages']}, sent to Azure {c['azure_pages']} "
          f"({pct(c['azure_pages'], c['pages'])}), graded {c['graded_pages']}")
    print(f"- graded values {c['right'] + c['wrong']}: right {c['right']}, wrong {c['wrong']} "
          f"(ungraded {c['values_ungraded']})")
    print(f"- relied on {relied} ({pct(relied, c['right'] + c['wrong'])}): verified "
          f"{c['verified_right'] + c['verified_wrong']}, confirmed by Azure "
          f"{c['confirmed_right'] + c['confirmed_wrong']}")
    print(f"- **relied on but wrong: {relied_wrong} of {relied} ({pct(relied_wrong, relied)})** "
          f"(verified {c['verified_wrong']}, confirmed {c['confirmed_wrong']})")
    print(f"- wrong values stopped: disputed {c['disputed_wrong']}, to a person "
          f"{c['person_wrong']}, of {c['wrong']}")
    print(f"- right values not relied on: disputed {c['disputed_right']}, to a person "
          f"{c['person_right']}, of {c['right']}")
    print(f"- Azure alone: has {pct(c['azure_has_right'], c['right'])} of right values, repeats "
          f"{c['azure_has_wrong']} of {c['wrong']} wrong values "
          f"({pct(c['azure_has_wrong'], c['wrong'])})")
    conf = c["sentences_confirmed_right"] + c["sentences_confirmed_unmatched"]
    print(f"- sentences on Azure pages {c['sentences']}: confirmed by Azure {conf}, of which not "
          f"word for word in the embedded text {c['sentences_confirmed_unmatched']} "
          f"({pct(c['sentences_confirmed_unmatched'], conf)}); unconfirmed "
          f"{c['sentences_unconfirmed_right'] + c['sentences_unconfirmed_unmatched']}")
    for k, e in ex.items():
        print(f"\n{k} ({len(e)}):")
        for x in e[:25]:
            print("   ", x)
    if a.json:
        with open(a.json, "w", encoding="utf-8") as f:
            json.dump({"bundle": a.bundle, "route": a.route, "counts": dict(c),
                       "examples": {k: e[:50] for k, e in ex.items()}}, f, indent=1)


if __name__ == "__main__":
    main()
