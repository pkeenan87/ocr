"""CER, WER, field precision and recall, and MiniCPM-OCR Jev flag metrics.

CER and WER come from ``eval/score.py``. The area under the ROC is the Mann-Whitney
AUC in ``eval/judge_calibrate.py`` (written for the unrelated TypeSafe judge; the
statistic is the same).
"""
import sys
from collections import Counter
from pathlib import Path

_EVAL = Path(__file__).resolve().parents[1]
if str(_EVAL) not in sys.path:
    sys.path.insert(0, str(_EVAL))
import score as scorer  # noqa: E402

from fields import by_kind, counter, pr_counts  # noqa: E402


def _auc(pos, neg):
    # Imported lazily so a metrics unit test does not have to load the judge client.
    from judge_calibrate import auc
    return auc(pos, neg)


def score_markdown(gt, md, strip_line_numbers=False):
    """``gt`` is a generate.py ground-truth dict, or ``{"pages": [{"text": ...}]}``."""
    return scorer.score(gt, md, strip_line_numbers=strip_line_numbers, keep_furniture=False)


def field_scores(ref_pages, hyp_pages):
    """Micro precision and recall over the zipped pages. Extra hyp pages count as extras."""
    n = max(len(ref_pages), len(hyp_pages))
    return field_scores_from_counters(
        [counter(ref_pages[i] if i < len(ref_pages) else "") for i in range(n)],
        [counter(hyp_pages[i] if i < len(hyp_pages) else "") for i in range(n)],
    )


def field_scores_from_counters(ref_counters, hyp_counters):
    """Micro precision and recall. A counter is already canonical ``(kind, value)`` counts."""
    ref_c, hyp_c = Counter(), Counter()
    for c in ref_counters:
        ref_c += c
    for c in hyp_counters:
        hyp_c += c
    total = pr_counts(ref_c, hyp_c)
    total["by_kind"] = by_kind(ref_c, hyp_c)
    return total


def disagreement(pages_a, pages_b):
    """Critical fields one engine has and the other does not. Not an accuracy number."""
    a, b = Counter(), Counter()
    n = max(len(pages_a), len(pages_b))
    per_page = []
    for i in range(n):
        ca = counter(pages_a[i] if i < len(pages_a) else "")
        cb = counter(pages_b[i] if i < len(pages_b) else "")
        a += ca
        b += cb
        per_page.append({
            "page": i + 1,
            "only_baseline": sorted(f"{k[0]}:{k[1]}" for k in (ca - cb)),
            "only_minicpm": sorted(f"{k[0]}:{k[1]}" for k in (cb - ca)),
            "shared": sum((ca & cb).values()),
        })
    return {
        "only_baseline": sum((a - b).values()),
        "only_minicpm": sum((b - a).values()),
        "shared": sum((a & b).values()),
        "by_kind": {
            kind: {
                "only_baseline": sum(n for k, n in (a - b).items() if k[0] == kind),
                "only_minicpm": sum(n for k, n in (b - a).items() if k[0] == kind),
                "shared": sum(n for k, n in (a & b).items() if k[0] == kind),
            }
            for kind in sorted({k[0] for k in list(a) + list(b)})
        },
        "pages": per_page,
    }


def _round(v):
    return None if v is None else round(v, 4)


def jev_report(regions, dropped_critical, thresholds):
    """AUROC, precision and recall of review flags, and critical-field errors caught.

    A flag is ``prob_yes < threshold`` on a row whose ``verdict`` is ``scored``.
    Higher ``prob_yes`` means the model thinks the candidate is correct, matching
    the MiniCPM-OCR Jev card. Rows that were not scored are left out of the ROC.
    """
    labeled = [r for r in regions if r.get("error") is not None and r.get("verdict") == "scored"
               and r.get("prob_yes") is not None]
    pos = [1.0 - r["prob_yes"] for r in labeled if r["error"]]
    neg = [1.0 - r["prob_yes"] for r in labeled if not r["error"]]
    rows = []
    for t in thresholds:
        flagged = [r for r in labeled if r["prob_yes"] < t]
        tp = sum(1 for r in flagged if r["error"])
        fp = len(flagged) - tp
        errors = len(pos)
        rows.append({
            "threshold": t,
            "precision": _round(tp / (tp + fp) if tp + fp else None),
            "recall": _round(tp / errors if errors else None),
            "flagged": len(flagged),
            "errors": errors,
            "critical_errors_caught": _round(_catch(regions, dropped_critical, t)),
        })
    critical_n = sum(len(r.get("critical_errors") or []) for r in regions) + len(dropped_critical)
    return {
        "regions": len(regions),
        "labeled": len(labeled),
        "errors": len(pos),
        "clean": len(neg),
        "unscored": sum(1 for r in regions if r.get("verdict") != "scored"),
        "auroc": _auc(pos, neg),
        "thresholds": rows,
        "critical_errors": critical_n,
        "dropped_critical_unpaired": len(dropped_critical),
        "image_scope": sorted({r.get("image_scope") or "unknown" for r in regions}),
    }


def _catch(regions, dropped, threshold):
    errors = [(r, e) for r in regions for e in (r.get("critical_errors") or [])]
    total = len(errors) + len(dropped)
    if not total:
        return None
    caught = 0
    for region, _err in errors:
        if region.get("verdict") == "scored" and region.get("prob_yes") is not None and region["prob_yes"] < threshold:
            caught += 1
    # Unpaired reference fields have no region to flag, so they count as missed.
    return caught / total


def public_fields(field_report, limit=40):
    """Drop nothing: the caller decides. This only caps list length for the JSON."""
    out = {k: field_report[k] for k in ("tp", "ref", "hyp", "precision", "recall")}
    out["by_kind"] = {
        k: {kk: vv for kk, vv in v.items() if kk != "misses" and kk != "extras"}
        | {"misses": v["misses"][:limit], "extras": v["extras"][:limit]}
        for k, v in field_report.get("by_kind", {}).items()
    }
    out["misses"] = field_report["misses"][:limit]
    out["extras"] = field_report["extras"][:limit]
    return out
