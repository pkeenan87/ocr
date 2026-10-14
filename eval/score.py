#!/usr/bin/env python3
"""Score an OCR Markdown output against an eval/generate.py ground-truth file.

usage: score.py <gt.json> <output.md> [--strip-line-numbers] [--keep-furniture]

Importable: score(gt, md_text, strip_line_numbers=False) -> dict. Metrics: CER (per page and
overall, micro-averaged over ground-truth characters) with its substitutions, deletions and
insertions; WER; both also *strict* (no NFKC, quotes and dashes not unified: what a reader copying
the text gets); token recall, Bates accuracy (page marker
`<!-- page N · Bates X -->` against the ground truth) and, for transcript4up, exact line accuracy.
Both sides pass through the same normalizer (Markdown/HTML table syntax removed, quotes and dashes
unified, whitespace collapsed). Page furniture listed in the ground truth (headers, footers, Bates
strings) is removed from the model output before comparison, so keeping or dropping it is neutral.
"""
import argparse
import collections, html, json, re, sys, unicodedata

try:
    from rapidfuzz.distance import Levenshtein as _lev

    def edit_distance(a, b):
        return _lev.distance(a, b)

    def edit_counts(ref, out):
        """(substitutions, deletions, insertions) turning `ref` into `out`; works on strings or
        word lists. A deletion is reference text the output lacks, an insertion is extra output."""
        ops = collections.Counter(op.tag for op in _lev.editops(ref, out))
        return ops["replace"], ops["delete"], ops["insert"]
except ImportError:  # numpy DP fallback, row by row
    def edit_counts(ref, out):
        return None
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


class PageError(ValueError):
    """Output whose page markers repeat or go backwards: not scoreable."""


def split_pages(md, preamble=False):
    """-> {page_no: (body, marker_extra)}, and with `preamble` also the text before the first
    marker. Markers must be unique and increasing, or this raises PageError."""
    md = strip_front_matter(md)
    ms = list(MARKER.finditer(md))
    if not ms:
        return ({1: (md, "")}, "") if preamble else {1: (md, "")}
    out = {}
    last = 0
    for i, m in enumerate(ms):
        n = int(m.group(1))
        if n <= last:
            raise PageError(f"page marker {n} after page {last}")
        last = n
        end = ms[i + 1].start() if i + 1 < len(ms) else len(md)
        out[n] = (md[m.end():end], m.group(2))
    return (out, md[:ms[0].start()]) if preamble else out


def _bounded_count(hay, token):
    """Occurrences of `token` in `hay` that are not part of a longer word or number:
    `$3,000` is not in `$3,000,000`, nor `12` in `123`."""
    pat = r"(?<![0-9A-Za-z])" + re.escape(token) + r"(?![0-9A-Za-z])(?![.,]\d)"
    if token[:1].isdigit():
        pat = r"(?<![.,]\d)" + pat
    return len(re.findall(pat, hay))


def _lcs(a, b):
    """Length of the longest common subsequence of two line lists (ordered, one-to-one)."""
    prev = [0] * (len(b) + 1)
    for x in a:
        cur = [0]
        for j, y in enumerate(b):
            cur.append(prev[j] + 1 if x == y else max(prev[j + 1], cur[j]))
        prev = cur
    return prev[-1]


def _html_tables(text):
    def table(m):
        rows = []
        for tr in re.findall(r"<tr[^>]*>(.*?)</tr>", m.group(0), re.S | re.I):
            cells = re.findall(r"<t[dh][^>]*>(.*?)</t[dh]>", tr, re.S | re.I)
            rows.append(" | ".join(re.sub(r"<[^>]+>", " ", c).strip() for c in cells))
        return "\n" + "\n".join(rows) + "\n"
    return re.sub(r"<table.*?</table>", table, text, flags=re.S | re.I)


def normalize_lines(text, strip_line_numbers=False, furniture=(), strict=False):
    """Plain-text lines, each whitespace-collapsed, empty lines dropped. `strict` keeps the
    characters as written (NFC only, quotes and dashes as they are); markup is removed either way."""
    text = unicodedata.normalize("NFC" if strict else "NFKC", text)
    text = _html_tables(text)
    text = re.sub(r"<br\s*/?>", " ", text, flags=re.I)
    text = re.sub(r"<!--.*?-->", "", text, flags=re.S)
    text = re.sub(r"</?[a-zA-Z][^>]*>", " ", text)
    text = html.unescape(text)
    if not strict:
        text = text.translate(QUOTES)
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
        def nf(f):
            f = unicodedata.normalize("NFC" if strict else "NFKC", f)
            return f if strict else f.translate(QUOTES)
        fur = [" ".join(nf(f).split()) for f in furniture]
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
    pages, pre = split_pages(md, preamble=True)
    cer_pages, dist_sum, len_sum = [], 0, 0
    # Strict CER, WER (normalized and strict), and the character edits by kind.
    m = collections.Counter()
    # Text outside the reference pages (before the first marker, or on pages the reference does
    # not have) is all insertions.
    extra_pages = [n for n in pages if not 1 <= n <= len(gt["pages"])]
    inserted = len(normalize(pre)) + sum(
        len(normalize(pages[n][0], strip_line_numbers=strip_line_numbers)) for n in extra_pages)
    dist_sum += inserted
    pre_strict = normalize(pre, strict=True)
    m["dist_strict"] += len(pre_strict) + sum(
        len(normalize(pages[n][0], strip_line_numbers=strip_line_numbers, strict=True))
        for n in extra_pages)
    m["ins"] += inserted
    extra_words = normalize(pre).split() + [
        w for n in extra_pages for w in normalize(pages[n][0], strip_line_numbers=strip_line_numbers).split()]
    m["wdist"] += len(extra_words)
    m["wdist_strict"] += len(extra_words)
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
        ops = edit_counts(ref, out)
        if ops:
            m["subs"] += ops[0]
            m["dels"] += ops[1]
            m["ins"] += ops[2]
        out_s = " ".join(normalize_lines(body, strip_line_numbers=strip_line_numbers, furniture=fur, strict=True))
        ref_s = " ".join(normalize_lines(gp["text"], strict=True))
        m["dist_strict"] += edit_distance(ref_s, out_s)
        m["len_strict"] += len(ref_s)
        m["wdist"] += edit_distance(ref.split(), out.split())
        m["wlen"] += len(ref.split())
        m["wdist_strict"] += edit_distance(ref_s.split(), out_s.split())
        m["wlen_strict"] += len(ref_s.split())
        # tokens are searched in the page body plus its marker (Bates may live only in the marker)
        hay = normalize(body) + " " + normalize(extra)
        # Each token must appear as itself, as often as the reference lists it.
        for t, k in collections.Counter(gp["tokens"]).items():
            tok_total += k
            tok_found += min(k, _bounded_count(hay, t))
        if gp.get("bates"):
            bates_total += 1
            bm = re.search(r"Bates\s+(\S+)", extra)
            bates_ok += bool(bm and bm.group(1) == gp["bates"])
        if gp.get("kind") == "transcript":
            # In order and one-to-one: a line read twice or out of order counts once.
            numbered = [ln for ln in ref_lines if re.match(r"\d{1,2} ", ln)]
            tl_total += len(numbered)
            tl_ok += _lcs(numbered, out_lines)
    res = {
        "family": gt.get("family"), "seed": gt.get("seed"), "pages": len(gt["pages"]),
        "pages_in_output": len(pages), "inserted_chars": inserted,
        "cer_strict": round(m["dist_strict"] / max(m["len_strict"], 1), 4),
        "wer": round(m["wdist"] / max(m["wlen"], 1), 4),
        "wer_strict": round(m["wdist_strict"] / max(m["wlen_strict"], 1), 4),
        **{k: m[k] for k in ("dist_strict", "len_strict", "wdist", "wlen", "wdist_strict",
                             "wlen_strict", "subs", "dels", "ins")},
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
