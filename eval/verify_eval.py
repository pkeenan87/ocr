"""How far `--verify-json` can be trusted, measured against the PDFs' embedded text.

usage:
    spike/.venv/bin/python -I eval/verify_eval.py --engine apple --out eval/out/verify [--skip-run]
        [--verify-fields flagged|all] [--set eval/public_set.json] [--pdfs eval/out/public/pdfs]
        [--use-real-config]

On every page with embedded text, the PDF's own text is the reference. Four scans per document:
- `user`: `--verify-json` as a user runs it (fields, word error rate, values verified, pages to
  Azure).
- `truth`: read without the embedded text (`--use-text-layer never`: no correction from it), and
  verified with it; values are graded by the embedded text.
- `blind`: the same reading, with the embedded text hidden from verify as well
  (`OCR_VERIFY_IGNORE_EMBEDDED=1`), as on a scan: the second reader decides alone. Its statuses
  are graded by the truth run, whose OCR is the same.
- `layer`: `--model apple --use-text-layer always`, whose values are the embedded text's own.

Pages whose embedded text is an invisible OCR layer are not graded.

Reported for pages with embedded text:
- field precision (OCR values the embedded text has), recall (embedded values the OCR has), F1;
- word error rate of the OCR against the embedded text (order-sensitive: a header the PDF
  stores elsewhere counts as moved words);
- status accuracy of the blind run: how often `verified` is wrong, how often `check` is a false
  alarm, how many wrong values `check` catches;
- sentences that independent readers agreed on but the embedded text does not have.
And for the user run: pages sent to Azure, and values verified.

The embedded text is not perfect ground truth: old scans carry their own bad OCR layer, and a
footnote marker can stick to an amount. Read the listed examples before trusting a number.
Every engine runs in the isolated state `eval/out/state/` unless `--use-real-config`.
"""

import argparse
import collections
import json
import os
import re
import subprocess
import sys

import pypdfium2 as pdfium
from rapidfuzz.distance import Levenshtein

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PAGE = re.compile(r"<!-- page (\d+)[^>]*-->")


def scan(ocr, engine, pdf, out_md, pages, extra, env):
    args = [ocr, "scan", "--input-file", pdf, "--output-file", out_md, "--model", engine,
            "--overwrite", "--quiet", "--verify-json", *extra]
    if pages:
        args += ["--pages", pages]
    r = subprocess.run(args, env=env, capture_output=True, text=True)
    # 0 clean, 1 flagged pages: both are results.
    if r.returncode not in (0, 1):
        sys.exit(f"{' '.join(args)} failed ({r.returncode}): {r.stderr.strip()[-400:]}")


def run_all(a, docs):
    env = dict(os.environ)
    if not a.use_real_config:
        env["OCR_CONFIG_DIR"] = os.path.join(ROOT, "eval/out/state/config")
        env["OCR_STATE_DIR"] = os.path.join(ROOT, "eval/out/state/state")
    blind = dict(env, OCR_VERIFY_IGNORE_EMBEDDED="1")
    mode = ["--verify-fields", a.verify_fields]
    # `user` is the scan as users run it. `truth` and `blind` read the pages without the embedded
    # text (`--use-text-layer never`, so no correction from it either), which gives both the same
    # OCR; only `truth`'s verify sees the embedded text, so it grades `blind`'s statuses.
    never = ["--use-text-layer", "never"]
    for kind, engine, extra, e in [
        ("user", a.engine, mode, env),
        ("truth", a.engine, mode + never, env),
        ("blind", a.engine, mode + never, blind),
        ("layer", "apple", ["--use-text-layer", "always"], env),
    ]:
        out = os.path.join(a.run, kind)
        os.makedirs(out, exist_ok=True)
        for d in docs:
            print(f"{kind}: {d['id']}", file=sys.stderr)
            scan(a.ocr, engine, os.path.join(a.pdfs, d["id"] + ".pdf"),
                 os.path.join(out, d["id"] + ".md"), d.get("pages_arg"), extra, e)


def load(a, kind, doc):
    with open(os.path.join(a.run, kind, doc + ".verify.json")) as f:
        return json.load(f)


def pages_md(path):
    with open(path) as f:
        md = f.read()
    body = md.split("\n---\n", 1)[1] if md.startswith("---\n") else md
    parts = PAGE.split(body)
    return {int(parts[i]): parts[i + 1] for i in range(1, len(parts) - 1, 2)}


def words(text):
    text = re.sub(r"<[^>]+>|\|[-: |]+\||[|#*_`]", " ", text)
    return [w for w in (re.sub(r"[^0-9a-z§¶$%]", "", x.lower()) for x in text.split()) if w]


def words_strict(text):
    """Words as written: case, punctuation, decimal points and signs kept; only Markdown and HTML
    markup removed, and a hyphen at a line end joined."""
    text = re.sub(r"<[^>]+>|\|[-: |]+\||[|#*_`]", " ", text)
    text = re.sub(r"-[ \t]*\r?\n\s*", "-", text)
    return text.split()


def field_id(f):
    """A value's identity: parsed value when there is one, else the text squashed."""
    v = f.get("value")
    if v is not None:
        return (f["kind"], str(v))
    return (f["kind"], re.sub(r"[^0-9a-z§¶$%]", "", f["text"].lower()))


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--engine", default="apple")
    p.add_argument("--out", default=os.path.join(ROOT, "eval/out/verify"))
    p.add_argument("--set", default=os.path.join(ROOT, "eval/public_set.json"))
    p.add_argument("--pdfs", default=os.path.join(ROOT, "eval/out/public/pdfs"))
    p.add_argument("--ocr", default=os.path.join(ROOT, "target/release/ocr"))
    p.add_argument("--verify-fields", choices=["flagged", "all"], default="flagged")
    p.add_argument("--skip-run", action="store_true")
    p.add_argument("--use-real-config", action="store_true")
    a = p.parse_args()
    # Flagged runs keep the old path, so earlier results stay where they were.
    a.run = os.path.join(a.out, a.engine + ("" if a.verify_fields == "flagged" else "-all"))
    with open(a.set) as f:
        docs = json.load(f)["documents"]
    if not a.skip_run:
        run_all(a, docs)

    c = collections.Counter()
    examples = collections.defaultdict(list)
    edits = ref_words = 0
    strict_edits = strict_ref = 0
    dropped = []  # (doc, page, reference words the OCR lacks, longest run of them)
    for d in docs:
        doc = d["id"]
        user, truth, blind = load(a, "user", doc), load(a, "truth", doc), load(a, "blind", doc)
        layer = load(a, "layer", doc)
        c["pages"] += len(user["pages"])
        c["azure_pages"] += user["summary"]["pages_to_check"]
        c["verified"] += user["summary"]["verified"]
        c["fields"] += user["summary"]["fields"]
        md = pages_md(os.path.join(a.run, "user", doc + ".md"))
        pdf = pdfium.PdfDocument(os.path.join(a.pdfs, doc + ".pdf"))
        lp = {p["page"]: p for p in layer["pages"]}
        for pu, pt, pb in zip(user["pages"], truth["pages"], blind["pages"]):
            # An invisible OCR layer is another OCR's reading, not a reference.
            if not pt["embedded_text"] or pt.get("ocr_text_layer"):
                continue
            n = pt["page"]
            c["graded_pages"] += 1
            # Fields: precision and recall against the embedded text's own values.
            ocr = collections.Counter(field_id(f) for f in pu["fields"] if f["kind"] != "bates")
            ref = collections.Counter(
                field_id(f) for f in lp.get(n, {"fields": []})["fields"] if f["kind"] != "bates")
            hit = sum((ocr & ref).values())
            c["field_hits"] += hit
            c["field_ocr"] += sum(ocr.values())
            c["field_ref"] += sum(ref.values())
            # Word error rate against the embedded text.
            ref_w = words(pdf[n - 1].get_textpage().get_text_range())
            ocr_w = words(md.get(n, ""))
            edits += Levenshtein.distance(ocr_w, ref_w)
            ref_words += len(ref_w)
            ref_s = words_strict(pdf[n - 1].get_textpage().get_text_range())
            strict_edits += Levenshtein.distance(words_strict(md.get(n, "")), ref_s)
            strict_ref += len(ref_s)
            # Deleted runs: reference words in a row the OCR does not have (a dropped line or
            # paragraph), as opposed to scattered misreads or a different reading order.
            longest = deleted = 0
            for op in Levenshtein.editops(ref_w, ocr_w):
                if op.tag == "delete":
                    deleted += 1
            # A run that appears elsewhere on the OCR page (8 words in a row) moved, not dropped.
            have = " " + " ".join(ocr_w) + " "
            for tag, i1, i2, j1, j2 in Levenshtein.opcodes(ref_w, ocr_w):
                if tag == "delete" and not any(
                        " " + " ".join(ref_w[k:k + 8]) + " " in have for k in range(i1, i2 - 7)):
                    longest = max(longest, i2 - i1)
            c["words_deleted"] += deleted
            if longest >= 12:
                dropped.append((doc, n, deleted, longest))
            # Status accuracy of the blind run, graded by the truth run.
            why_t = {(f["kind"], f["text"], f["line"]): f["why"] for f in pt["fields"]}
            for f in pb["fields"]:
                w = why_t.get((f["kind"], f["text"], f["line"]))
                if w is None or not ({"matches_embedded_text", "not_in_embedded_text"} & set(w)):
                    continue
                right = "matches_embedded_text" in w
                c["right" if right else "wrong"] += 1
                if f["status"] == "verified":
                    c["verified_right" if right else "verified_wrong"] += 1
                    if not right:
                        examples["verified but wrong"].append((doc, n, f["text"]))
                elif f["status"] == "check":
                    c["check_right" if right else "check_wrong"] += 1
                    # A page-level reason alone (suspect page, missing text) or the value itself.
                    if right and set(f["why"]) <= {"suspect_page"}:
                        c["check_right_page_only"] += 1
            # Sentences independent readers agreed on that the embedded text lacks.
            at, ab = pt.get("text_agreement"), pb.get("text_agreement")
            if at and ab and ab["independent"]:
                un_t = {(u["line"], u["text"]) for u in at["unagreed"]}
                un_b = {(u["line"], u["text"]) for u in ab["unagreed"]}
                c["agreed_blind"] += ab["agreed_spans"]
                for u in un_t - un_b:
                    c["agreed_blind_wrong"] += 1
                    examples["sentence agreed but not in embedded text"].append((doc, n, u[1][:90]))

    def pct(x, y):
        return f"{x / y:.1%}" if y else "n/a"

    prec = c["field_hits"] / max(1, c["field_ocr"])
    rec = c["field_hits"] / max(1, c["field_ref"])
    f1 = 2 * prec * rec / max(1e-9, prec + rec)
    print(f"# verify eval: {a.engine}, --verify-fields {a.verify_fields}\n")
    print(f"- pages {c['pages']}, graded (embedded text) {c['graded_pages']}")
    print(f"- user run: values verified {c['verified']} of {c['fields']}; "
          f"pages to Azure {c['azure_pages']}")
    print(f"- fields vs embedded text: precision {prec:.1%}, recall {rec:.1%}, F1 {f1:.1%}")
    print(f"- word error rate vs embedded text: {pct(edits, ref_words)} ({ref_words} words); "
          f"strict (case, punctuation, decimals kept) {pct(strict_edits, strict_ref)}; "
          f"reference words deleted {c['words_deleted']}")
    print(f"- pages with a run of 12+ reference words missing and found nowhere on the OCR page "
          f"(a dropped line or paragraph): {len(dropped)}")
    for x in sorted(dropped, key=lambda x: -x[3])[:10]:
        print("   ", x)
    print(f"- blind statuses: verified {c['verified_right'] + c['verified_wrong']}, "
          f"of which wrong {c['verified_wrong']} "
          f"({pct(c['verified_wrong'], c['verified_right'] + c['verified_wrong'])})")
    print(f"- blind statuses: wrong values {c['wrong']}, caught as check {c['check_wrong']} "
          f"({pct(c['check_wrong'], c['wrong'])}); right values {c['right']}, false alarms "
          f"{c['check_right']} ({pct(c['check_right'], c['right'])}), of which "
          f"{c['check_right'] - c['check_right_page_only']} from the value itself "
          f"({pct(c['check_right'] - c['check_right_page_only'], c['right'])}) and the rest "
          f"from a page check")
    print(f"- sentences agreed by independent readers {c['agreed_blind']}, not in the embedded "
          f"text {c['agreed_blind_wrong']} ({pct(c['agreed_blind_wrong'], c['agreed_blind'])})")
    for k, ex in examples.items():
        print(f"\n{k} ({len(ex)}):")
        for e in ex[:15]:
            print("   ", e)
    with open(os.path.join(a.run, "summary.json"), "w") as f:
        json.dump(dict(c), f, indent=1)


if __name__ == "__main__":
    main()
