"""Critical-value extraction for the MiniCPM comparison.

Patterns follow ``src/verify.rs`` (amounts, dates, federal and labeled case numbers,
section and paragraph signs). Worded "Section" / "Paragraph" / "Article" refs are an
extra pattern: the Rust extractor only has the sign form. Typed comparison for amounts,
dates and dockets reuses ``eval/holdout/score.py``.
"""
import importlib.util
import re
import sys
from collections import Counter
from pathlib import Path

_EVAL = Path(__file__).resolve().parents[1]
if str(_EVAL) not in sys.path:
    sys.path.insert(0, str(_EVAL))

_spec = importlib.util.spec_from_file_location("holdout_score", _EVAL / "holdout" / "score.py")
_holdout = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_holdout)

# src/verify.rs AMOUNT, DATE_*, DOCKET, LABELED_DOCKET, CITATION.
AMOUNT = re.compile(
    r"(?i)\$\s?-?[\dOolISB][\dOolISB,]*(?:\.[\dOolISB]+)?"
    r"(?:\s(?:thousand|million|billion|trillion)\b|[KMB]\b)?"
)
DATE_NUMERIC = re.compile(r"\b[0-9OIlQ]{1,2}/[0-9OIlQ]{1,2}/[0-9OIlQ]{2,4}\b|\b\d{4}-\d{2}-\d{2}\b")
_MONTHS = (
    "January|February|March|April|May|June|July|August|September|October|November|December|"
    "Jan|Feb|Mar|Apr|Jun|Jul|Aug|Sept|Sep|Oct|Nov|Dec"
)
DATE_WORDS = re.compile(
    rf"\b(?:(?P<m1>{_MONTHS})\.?\s+(?P<d1>[0-9OIl]{{1,2}})(?:st|nd|rd|th)?(?:,\s*|\s+)(?P<y1>[0-9OIlQ]{{4}})"
    rf"|(?P<d2>\d{{1,2}})(?:st|nd|rd|th)?\s+(?:day\s+of\s+|of\s+)?(?P<m2>{_MONTHS})\.?,?\s+(?P<y2>\d{{4}}))\b",
    re.I,
)
DOCKET = re.compile(
    r"(?i)\b(?:[\dOIl]{1,2}:)?[\dOIl]{2}-(?:cv|cr|mc|md|bk|ap|mj|po|sw|civ|cvd)-[\dOIlSB]{2,6}"
    r"(?:-[A-Z]{1,4})*\b"
)
LABELED_DOCKET = re.compile(
    r"(?i)\b(?:case|civil action|c\.a\.|index|cause)\s*(?:no|number|#)\.?:?\s*"
    r"([A-Za-z0-9][\w:./-]*\d(?:\s(?:civ|cv|cr)\.?\s*\d{1,6})?(?:[- ][A-Z]{2,4}\b)?)"
)
CITATION = re.compile(r"(?:§§?|¶¶?)\s?\d(?:[\w\-]|\.(?:[\w\-]))*(?:\([A-Za-z0-9]{1,4}\))*")
WORDED_REF = re.compile(
    r"(?i)\b(?P<kind>section|sections|sec\.|paragraph|paragraphs|para\.|article|articles)"
    r"\s+(?P<n>\d+(?:\.\d+)*)"
)

CRITICAL_KINDS = ("amount", "date", "docket", "citation", "section", "paragraph")


def _overlaps(spans, start, end):
    return any(s < end and start < e for s, e in spans)


def _worded_kind(word):
    w = word.lower().rstrip(".")
    if w.startswith("para"):
        return "paragraph"
    if w.startswith("art"):
        return "section"
    return "section"


def extract(text):
    """Yield (kind, surface) in reading order. ``section`` covers "Section N" and "Article N"."""
    found = []
    spans = []
    for rx, kind in ((AMOUNT, "amount"), (DATE_NUMERIC, "date"), (DATE_WORDS, "date"), (DOCKET, "docket")):
        for m in rx.finditer(text):
            if _overlaps(spans, m.start(), m.end()):
                continue
            found.append((m.start(), kind, m.group(0)))
            spans.append((m.start(), m.end()))
    for m in LABELED_DOCKET.finditer(text):
        n = m.start(1)
        end = m.end(1)
        if _overlaps(spans, n, end):
            continue
        surface = m.group(1).strip().rstrip(".,;:")
        if sum(c.isdigit() for c in surface) < 4:
            continue
        found.append((n, "docket", surface))
        spans.append((n, end))
    for m in CITATION.finditer(text):
        if _overlaps(spans, m.start(), m.end()):
            continue
        surface = m.group(0).rstrip(".,-")
        kind = "paragraph" if "¶" in surface else "citation"
        if kind == "citation" and surface.startswith("§"):
            kind = "section"
        found.append((m.start(), kind, surface))
        spans.append((m.start(), m.end()))
    for m in WORDED_REF.finditer(text):
        if _overlaps(spans, m.start(), m.end()):
            continue
        kind = _worded_kind(m.group("kind"))
        if kind == "section" and m.group("kind").lower().startswith("art"):
            surface = f"Article {m.group('n')}"
        else:
            surface = m.group(0)
        found.append((m.start(), kind, surface))
        spans.append((m.start(), m.end()))
    found.sort()
    return [(k, s) for _, k, s in found]


def canonical(kind, text):
    """(kind, typed value) comparable across engines. Amounts are signed cents."""
    if kind == "amount":
        return _holdout.typed("amount", text)
    if kind == "date":
        return _holdout.typed("date", text)
    if kind == "docket":
        return _holdout.typed("docket", text)
    if kind in ("section", "paragraph", "citation"):
        if kind == "section" and text.lower().startswith("article"):
            m = re.search(r"\d+(?:\.\d+)*", text)
            return ("section", f"article:{m.group(0)}" if m else _holdout.chars(text))
        m = re.search(r"\d+(?:\.\d+)*", text)
        prefix = "para" if kind == "paragraph" else "sec"
        return (kind if kind != "citation" else "section", f"{prefix}:{m.group(0)}" if m else _holdout.chars(text))
    return (kind, _holdout.chars(text))


def counter(text):
    return Counter(canonical(k, s) for k, s in extract(text) if k in CRITICAL_KINDS or k == "citation")


def pr_counts(ref, hyp):
    """Multiset precision and recall. ``ref`` and ``hyp`` are Counters of canonical fields."""
    if not isinstance(ref, Counter):
        ref = Counter(ref)
    if not isinstance(hyp, Counter):
        hyp = Counter(hyp)
    tp = sum((ref & hyp).values())
    ref_n, hyp_n = sum(ref.values()), sum(hyp.values())
    return {
        "tp": tp,
        "ref": ref_n,
        "hyp": hyp_n,
        "precision": round(tp / hyp_n, 4) if hyp_n else None,
        "recall": round(tp / ref_n, 4) if ref_n else None,
        "misses": sorted(f"{k[0]}:{k[1]}" for k, n in (ref - hyp).items() for _ in range(n)),
        "extras": sorted(f"{k[0]}:{k[1]}" for k, n in (hyp - ref).items() for _ in range(n)),
    }


def by_kind(ref, hyp):
    out = {}
    kinds = {k[0] for k in list(ref) + list(hyp)}
    for kind in sorted(kinds):
        out[kind] = pr_counts(
            Counter({k: n for k, n in ref.items() if k[0] == kind}),
            Counter({k: n for k, n in hyp.items() if k[0] == kind}),
        )
    return out
