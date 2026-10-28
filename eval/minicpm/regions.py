"""Split a page into text, table and formula regions and align them to a reference.

MiniCPM-OCR Jev scores one crop plus one candidate. The current engine returns page
Markdown, not crops. Without a layout file the harness still splits the text, and the
image it sends is the whole page (recorded as ``image_scope: full_page``). That is not
how Jev was trained. A layout JSON supplies pixel boxes and the image is a real crop.
"""
import sys
from pathlib import Path

_EVAL = Path(__file__).resolve().parents[1]
if str(_EVAL) not in sys.path:
    sys.path.insert(0, str(_EVAL))
import score as scorer  # noqa: E402

from fields import counter, pr_counts  # noqa: E402

_TABLE = {"table", "chart"}
_FORMULA = {"display_formula", "inline_formula", "formula_number", "algorithm"}
_SKIP = {"image", "header_image", "footer_image"}


class Block:
    def __init__(self, category, text, label=None, bbox=None):
        self.category = category
        self.text = text
        self.label = label
        self.bbox = bbox


def category_of(text, label=None):
    label = (label or "").lower()
    if label in _SKIP:
        return None
    if label in _TABLE or "<table" in text.lower() or text.lstrip().startswith("|"):
        return "table"
    if label in _FORMULA or "\\frac" in text or "$$" in text:
        return "formula"
    return "text"


def split_blocks(text):
    """Paragraphs (blank-line separated), HTML tables and pipe tables, in order."""
    import re
    parts = []
    pos = 0
    for m in re.finditer(r"<table\b.*?</table>", text, re.I | re.S):
        parts.extend(_plain_blocks(text[pos:m.start()]))
        cat = category_of(m.group(0), "table")
        if cat and m.group(0).strip():
            parts.append(Block(cat, m.group(0).strip(), "table"))
        pos = m.end()
    parts.extend(_plain_blocks(text[pos:]))
    return parts


def _plain_blocks(text):
    blocks = []
    buf, mode = [], None

    def flush():
        if not buf:
            return
        body = "\n".join(buf).strip()
        buf.clear()
        if not body:
            return
        cat = "table" if mode == "mdtable" else category_of(body)
        if cat:
            blocks.append(Block(cat, body, mode or "text"))

    for ln in text.splitlines():
        pipe = ln.strip().startswith("|")
        if not ln.strip():
            flush()
            mode = None
            continue
        if pipe:
            if mode != "mdtable":
                flush()
                mode = "mdtable"
            buf.append(ln)
        else:
            if mode == "mdtable":
                flush()
            mode = "text"
            buf.append(ln)
    flush()
    return blocks


def _norm(text):
    return scorer.normalize(text)


def align(ref_blocks, hyp_blocks):
    """Pair hypothesis blocks with reference blocks. Returns hyp_index -> ref_index or None.

    Cost is the normalized edit distance, so a one-token misread still pairs.
    """
    n, m = len(ref_blocks), len(hyp_blocks)
    if n * m > 2500:
        # A page this fragmented is not worth a quadratic alignment. Zip in order.
        return {j: j if j < n else None for j in range(m)}
    rn = [_norm(b.text) for b in ref_blocks]
    hn = [_norm(b.text) for b in hyp_blocks]
    dp = [[0] * (m + 1) for _ in range(n + 1)]
    back = [[""] * (m + 1) for _ in range(n + 1)]
    for i in range(1, n + 1):
        dp[i][0] = dp[i - 1][0] + len(rn[i - 1]) + 1
        back[i][0] = "u"
    for j in range(1, m + 1):
        dp[0][j] = dp[0][j - 1] + len(hn[j - 1]) + 1
        back[0][j] = "l"
    for i in range(1, n + 1):
        for j in range(1, m + 1):
            dist = scorer.edit_distance(rn[i - 1], hn[j - 1])
            opts = (
                (dp[i - 1][j - 1] + dist, "d"),
                (dp[i - 1][j] + len(rn[i - 1]) + 1, "u"),
                (dp[i][j - 1] + len(hn[j - 1]) + 1, "l"),
            )
            dp[i][j], back[i][j] = min(opts)
    pair = {}
    i, j = n, m
    while i or j:
        step = back[i][j]
        if step == "d":
            pair[j - 1] = i - 1
            i, j = i - 1, j - 1
        elif step == "u":
            i -= 1
        else:
            pair[j - 1] = None
            j -= 1
    return pair


def _cer(ref, hyp):
    ref_n, hyp_n = _norm(ref), _norm(hyp)
    if not ref_n and not hyp_n:
        return 0.0
    dist = scorer.edit_distance(ref_n, hyp_n)
    return dist / max(len(ref_n), 1)


def label_page(ref_text, hyp_text, layout_blocks=None):
    """Regions of the hypothesis page, each with an error label when ``ref_text`` is set.

    ``layout_blocks`` is a list of ``{label, bbox}`` in reading order. They are zipped onto
    hypothesis blocks only when the counts match; otherwise the text split stands and
    ``bbox`` stays None.
    """
    hyp = split_blocks(hyp_text or "")
    if layout_blocks and len(layout_blocks) == len(hyp):
        for block, lay in zip(hyp, layout_blocks):
            block.bbox = lay.get("bbox")
            cat = category_of(block.text, lay.get("label"))
            if cat:
                block.category = cat
                block.label = lay.get("label")
    ref = split_blocks(ref_text) if ref_text is not None else None
    pair = align(ref, hyp) if ref is not None else {}
    paired_ref = {v for v in pair.values() if v is not None}
    regions = []
    for j, block in enumerate(hyp):
        rec = {
            "region": j,
            "category": block.category,
            "label": block.label,
            "bbox": block.bbox,
            "chars": len(block.text),
            "text": block.text,
        }
        if ref is None:
            rec.update(error=None, cer=None, critical_errors=[])
        else:
            ri = pair.get(j)
            if ri is None:
                fields = pr_counts(counter(""), counter(block.text))
                cer = 1.0 if _norm(block.text) else 0.0
            else:
                fields = pr_counts(counter(ref[ri].text), counter(block.text))
                cer = _cer(ref[ri].text, block.text)
            misses = [{"kind": item.split(":", 1)[0], "value": item.split(":", 1)[1], "side": "ref"}
                      for item in fields["misses"]]
            extras = [{"kind": item.split(":", 1)[0], "value": item.split(":", 1)[1], "side": "hyp"}
                      for item in fields["extras"]]
            rec.update(error=cer > 0 or bool(misses) or bool(extras), cer=round(cer, 4),
                       critical_errors=misses + extras, paired_ref=ri)
        regions.append(rec)
    dropped = []
    if ref is not None:
        for i, block in enumerate(ref):
            if i in paired_ref:
                continue
            for item in pr_counts(counter(block.text), counter(""))["misses"]:
                kind, _, value = item.partition(":")
                dropped.append({"kind": kind, "value": value, "side": "ref", "unpaired_ref": True})
    return regions, dropped


def crop_image(image, bbox, coord="px"):
    """Crop ``bbox`` as x1,y1,x2,y2. ``coord`` is ``px`` or ``norm`` (0-1)."""
    w, h = image.size
    x1, y1, x2, y2 = bbox
    if coord == "norm":
        x1, x2 = x1 * w, x2 * w
        y1, y2 = y1 * h, y2 * h
    x1, y1 = max(0, int(round(x1))), max(0, int(round(y1)))
    x2, y2 = min(w, int(round(x2))), min(h, int(round(y2)))
    if x2 <= x1 or y2 <= y1:
        return image
    return image.crop((x1, y1, x2, y2))


def load_layout(path, page):
    """Blocks for one page from a harness layout file or one SDK ``layouts/*.json``."""
    import json
    if path is None or not Path(path).exists():
        return None
    data = json.loads(Path(path).read_text())
    # SDK writes one file per page: source.page_index is 0-based.
    source = data.get("source") or {}
    if "blocks" in data and "pages" not in data:
        idx = source.get("page_index")
        if idx is not None and int(idx) + 1 != page:
            return None
        return [_lay(b) for b in data["blocks"] if (b.get("label") or "").lower() not in _SKIP]
    for entry in data.get("pages") or []:
        if int(entry.get("page") or 0) == page:
            coord = entry.get("coord") or data.get("coord") or "px"
            return [_lay(b, coord) for b in entry.get("blocks") or []
                    if (b.get("label") or "").lower() not in _SKIP]
    return None


def _lay(block, coord="px"):
    return {"label": block.get("label") or "text", "bbox": block.get("bbox"), "coord": block.get("coord") or coord}
