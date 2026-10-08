#!/usr/bin/env python3
"""Score an OCR Markdown output against an eval/generate.py ground-truth file.

usage: score.py <gt.json> <output.md> [--strip-line-numbers] [--keep-furniture]

Importable: score(gt, md_text, strip_line_numbers=False) -> dict. Metrics: CER (per page and
overall, micro-averaged over ground-truth characters), token recall, Bates accuracy (page marker
`<!-- page N · Bates X -->` against the ground truth) and, for transcript4up, exact line accuracy.
Both sides pass through the same normalizer (Markdown/HTML table syntax removed, quotes and dashes
unified, whitespace collapsed). Page furniture listed in the ground truth (headers, footers, Bates
strings) is removed from the model output before comparison, so keeping or dropping it is neutral.
"""
import argparse, html, json, re, sys, unicodedata

try:
    from rapidfuzz.distance import Levenshtein as _lev

    def edit_distance(a, b):
        return _lev.distance(a, b)
except ImportError:  # numpy DP fallback, row by row
    import numpy as np

    def edit_distance(a, b):
        if not a or not b:
            return max(len(a), len(b))
        bb = np.frombuffer(b.encode("utf-32-le"), dtype=np.uint32)
        prev = np.arange(len(b) + 1)
        for i, ch in enumerate(a, 1):
            sub = prev[:-1] + (bb != ord(ch))
            cur = np.empty_like(prev)
            cur[0] = i
            cur[1:] = np.minimum(sub, prev[1:] + 1)
            # insertions are a running dependency along the row
            for j in range(1, len(cur)):
                if cur[j - 1] + 1 < cur[j]:
                    cur[j] = cur[j - 1] + 1
            prev = cur
        return int(prev[-1])

MARKER = re.compile(r"<!--\s*page\s+(\d+)([^>]*?)-->")
QUOTES = str.maketrans({"‘": "'", "’": "'", "‚": "'", "“": '"', "”": '"', "„": '"',
                        "‐": "-", "‑": "-", "‒": "-", "–": "-", "—": "-", "―": "-",
                        "−": "-", " ": " "})


def strip_front_matter(md):
    if md.startswith("---\n"):
        end = md.find("\n---\n", 4)
        if end >= 0:
            return md[end + 5:]
    return md


def split_pages(md):
    """-> {page_no: (body, marker_extra)}. Text before the first marker is ignored."""
    md = strip_front_matter(md)
    ms = list(MARKER.finditer(md))
    if not ms:
        return {1: (md, "")}
    out = {}
    for i, m in enumerate(ms):
        end = ms[i + 1].start() if i + 1 < len(ms) else len(md)
        out[int(m.group(1))] = (md[m.end():end], m.group(2))
    return out


def _html_tables(text):
    def table(m):
        rows = []
        for tr in re.findall(r"<tr[^>]*>(.*?)</tr>", m.group(0), re.S | re.I):
            cells = re.findall(r"<t[dh][^>]*>(.*?)</t[dh]>", tr, re.S | re.I)
            rows.append(" | ".join(re.sub(r"<[^>]+>", " ", c).strip() for c in cells))
        return "\n" + "\n".join(rows) + "\n"
    return re.sub(r"<table.*?</table>", table, text, flags=re.S | re.I)


def normalize_lines(text, strip_line_numbers=False, furniture=()):
    """Plain-text lines, each whitespace-collapsed, empty lines dropped."""
    text = unicodedata.normalize("NFKC", text)
    text = _html_tables(text)
    text = re.sub(r"<br\s*/?>", " ", text, flags=re.I)
    text = re.sub(r"<!--.*?-->", "", text, flags=re.S)
    text = re.sub(r"</?[a-zA-Z][^>]*>", " ", text)
    text = html.unescape(text).translate(QUOTES)
    out = []
    for ln in text.split("\n"):
        s = ln.strip()
        if re.fullmatch(r"\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?", s):
            continue  # table separator row
        if s.startswith("|"):
            cells = [c.strip() for c in s.strip("|").split("|")]
            s = " | ".join(cells)
        s = re.sub(r"^#{1,6}\s+", "", s)
        s = re.sub(r"^[-*+]\s+", "", s)
        s = s.replace("**", "")
        s = re.sub(r"\\([\\`*_{}\[\]()#+\-.!|$])", r"\1", s)
        if strip_line_numbers:
            s = re.sub(r"^\d{1,2}\s+", "", s)
            if re.fullmatch(r"(\d{1,2}\s*)+", s):
                s = ""
        s = " ".join(s.split())
        if s:
            out.append(s)
    if furniture:
        fur = [" ".join(unicodedata.normalize("NFKC", f).translate(QUOTES).split()) for f in furniture]
        fur = sorted((f for f in fur if f), key=len, reverse=True)
        res = []
        for ln in out:
            for f in fur:
                ln = ln.replace(f, " ")
            ln = " ".join(ln.split())
            if ln:
                res.append(ln)
        out = res
    return out


def normalize(text, **kw):
    return " ".join(normalize_lines(text, **kw))


def score(gt, md, strip_line_numbers=False, keep_furniture=False):
    pages = split_pages(md)
    cer_pages, dist_sum, len_sum = [], 0, 0
    tok_found = tok_total = 0
    bates_ok = bates_total = 0
    tl_ok = tl_total = 0
    for i, gp in enumerate(gt["pages"], 1):
        body, extra = pages.get(i, ("", ""))
        fur = [] if keep_furniture else gp.get("furniture", [])
        out_lines = normalize_lines(body, strip_line_numbers=strip_line_numbers, furniture=fur)
        out = " ".join(out_lines)
        ref_lines = normalize_lines(gp["text"])
        ref = " ".join(ref_lines)
        d = edit_distance(ref, out)
        cer_pages.append(round(d / max(len(ref), 1), 4))
        dist_sum += d
        len_sum += len(ref)
        # tokens are searched in the page body plus its marker (Bates may live only in the marker)
        hay = normalize(body) + " " + normalize(extra)
        for t in gp["tokens"]:
            tok_total += 1
            tok_found += t in hay
        if gp.get("bates"):
            bates_total += 1
            m = re.search(r"Bates\s+(\S+)", extra)
            bates_ok += bool(m and m.group(1) == gp["bates"])
        if gp.get("kind") == "transcript":
            have = set(out_lines)
            for ln in ref_lines:
                if re.match(r"\d{1,2} ", ln):
                    tl_total += 1
                    tl_ok += ln in have
    res = {
        "family": gt.get("family"), "seed": gt.get("seed"), "pages": len(gt["pages"]),
        "pages_in_output": len(pages),
        "cer": round(dist_sum / max(len_sum, 1), 4), "cer_pages": cer_pages,
        "dist_sum": dist_sum, "len_sum": len_sum, "bates_ok": bates_ok, "bates_total": bates_total,
        "tl_ok": tl_ok, "tl_total": tl_total,
        "token_recall": round(tok_found / tok_total, 4) if tok_total else None,
        "tokens_found": tok_found, "tokens_total": tok_total,
        "bates_accuracy": round(bates_ok / bates_total, 4) if bates_total else None,
        "transcript_line_accuracy": round(tl_ok / tl_total, 4) if tl_total else None,
    }
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("gt")
    ap.add_argument("md")
    ap.add_argument("--strip-line-numbers", action="store_true",
                    help="strip leading 1-2 digit margin numbers from output lines (pleading family)")
    ap.add_argument("--keep-furniture", action="store_true", help="score headers/footers/Bates too")
    ap.add_argument("--diff-page", type=int, help="print normalized reference and output for page N")
    a = ap.parse_args()
    gt = json.load(open(a.gt))
    md = open(a.md).read()
    if a.diff_page:
        gp = gt["pages"][a.diff_page - 1]
        body = split_pages(md).get(a.diff_page, ("", ""))[0]
        print("REF:", normalize(gp["text"]), "\nOUT:", normalize(body, strip_line_numbers=a.strip_line_numbers,
                                                                  furniture=gp.get("furniture", [])))
        return
    r = score(gt, md, a.strip_line_numbers, a.keep_furniture)
    if len(r["cer_pages"]) > 20:
        r["cer_pages"] = r["cer_pages"][:20] + ["..."]
    json.dump(r, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main()
