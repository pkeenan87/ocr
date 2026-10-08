#!/usr/bin/env python3
"""Dev-only OCR output quality judge using TypeSafe AI's Jev decision model via Vercel AI Gateway.

NOT part of the ocr CLI. A text-only plausibility signal for Markdown outputs that have no ground
truth. Only ever send synthetic eval outputs or public documents -- never client files (JUDGE.md).

usage:
  judge.py <output.md> [--family F] [--json OUT] [--threshold T]
  judge.py --engine-compare A.md B.md [C.md ...] [--json OUT]

Key: AI_GATEWAY_API_KEY from the environment, else parsed from <repo>/.env. Never printed.
Endpoint: POST https://ai-gateway.vercel.sh/v1/evaluate (see JUDGE.md).
"""
import argparse, hashlib, json, os, re, sys, threading, time, urllib.error, urllib.request
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from score import split_pages  # noqa: E402  (also strips YAML front matter)

ENDPOINT = "https://ai-gateway.vercel.sh/v1/evaluate"
MODEL = "typesafe-ai/jev"
CACHE = os.path.join(HERE, "out", "judge", "cache.jsonl")
MAX_REQUESTS = 2000
# Jev's budget is ~32k tokens for state + questions, and each question is evaluated in isolation.
# 24k characters (~6-8k tokens) per chunk leaves ample room.
CHUNK_CHARS = 24000

LEGAL_OK = ("Legitimate legal-document features are NOT defects: pleading margin line numbers "
            "(1-28 at line starts), Bates numbers (e.g. ABC000123), deposition 'Q.'/'A.' lines "
            "with line numbers, numbered paragraphs or sections (1., 1.1, ARTICLE 2), legal "
            "citations (e.g. 28 U.S.C. § 1332, Fed. R. Civ. P. 12(b)(6)), case numbers, dollar "
            "amounts, dates, names, defined terms in quotes, and Markdown syntax.")

QUESTIONS = {
    "garbled": {
        "type": "boolean",
        "instructions": ("This is text produced by OCR from one page of a document. Does it contain "
                         "garbled text, i.e. OCR misreadings? " + LEGAL_OK),
        "criteria": {
            "true": ("Several words are misspelled non-words, have dropped or swapped letters, wrong "
                     "characters (rn/m, l/1, O/0), are truncated, or are wrongly merged, e.g. "
                     "'Cuestion 1.2 arout the newting' or 'concern ne perchase price'."),
            "false": ("Words are correctly spelled real words or legitimate names, numbers, codes "
                      "and citations; at most an isolated typo. " + LEGAL_OK),
        },
    },
    "interleaved": {
        "type": "boolean",
        "instructions": ("This is OCR text from one page. Were two text columns (or two side-by-side "
                         "blocks) merged line by line, so consecutive lines alternate between two "
                         "unrelated threads of text? " + LEGAL_OK),
        "criteria": {
            "true": ("Sentences break off mid-phrase and continue two lines later; adjacent lines "
                     "jump between two different topics or sentences, so the text reads as two "
                     "streams interleaved."),
            "false": ("Each sentence and paragraph continues naturally onto the next line; text "
                      "reads in a single coherent order. Short numbered lines, Q./A. testimony or "
                      "numbered sections that follow one another in order are normal. " + LEGAL_OK),
        },
    },
    "repetition": {
        "type": "boolean",
        "instructions": ("This is OCR text from one page. Does it contain a runaway repetition loop, "
                         "where the same phrase or line is repeated many times in a row by mistake?"),
        "criteria": {
            "true": ("The same phrase, sentence or line appears five or more times consecutively or "
                     "nearly consecutively, as a machine glitch."),
            "false": ("No phrase repeats abnormally. Ordinary recurring words, repeated headers, "
                      "recurring column values in a table, repeated 'Q.'/'A.' prefixes or "
                      "boilerplate that appears a few times are normal."),
        },
    },
    "table_broken": {
        "type": "boolean",
        "instructions": ("This is OCR text from one page that contains a table in Markdown (pipe) "
                         "format. Is the table broken or misaligned?"),
        "criteria": {
            "true": ("Rows have inconsistent numbers of cells, cell values are in the wrong columns "
                     "(e.g. a date under Amount, a description under Rate), pipes are missing so "
                     "cells run together, or several rows are merged into one."),
            "false": ("Every row has the same number of cells and each column holds consistent "
                      "kinds of values matching its header."),
        },
    },
    "readability": {
        "type": "score",
        "instructions": ("This is OCR text from one page of a document. Rate how readable and "
                         "correct it is as text. " + LEGAL_OK),
        "criteria": [
            "1: unreadable; mostly garbage, scrambled or looping text",
            "2: poor; frequent misread words or scrambled order, meaning often lost",
            "3: fair; understandable but with noticeable errors or disorder",
            "4: good; reads well with only a few minor errors",
            "5: excellent; clean, coherent, correctly spelled text in natural reading order",
        ],
    },
}
BOOL_QS = ["garbled", "interleaved", "repetition", "table_broken"]
# Calibrated flag thresholds (judge_calibrate.py, seeds 10-19; see JUDGE.md).
THRESHOLDS = {"garbled": 0.45, "interleaved": 0.35, "repetition": 0.55, "table_broken": 0.4}
READABILITY_FLAG = 3.5  # pages scoring below this are worth a look


# ---------------------------------------------------------------- client
def _load_key():
    k = os.environ.get("AI_GATEWAY_API_KEY")
    if k:
        return k.strip()
    path = os.path.join(REPO, ".env")
    try:
        with open(path) as f:
            for ln in f:
                ln = ln.strip()
                if ln.startswith("export "):
                    ln = ln[7:].strip()
                if ln.startswith("AI_GATEWAY_API_KEY") and "=" in ln:
                    return ln.split("=", 1)[1].strip().strip('"').strip("'")
    except OSError:
        pass
    sys.exit("judge: AI_GATEWAY_API_KEY not set and not found in .env")


class Jev:
    """Thread-safe client with a response cache, a request cap and usage counters."""

    def __init__(self, cache=CACHE, max_requests=MAX_REQUESTS, use_cache=True):
        self._key = None
        self.lock = threading.Lock()
        self.requests = self.cached = self.retries = 0
        self.input_tokens = self.output_tokens = 0
        self.cost = 0.0
        self.max_requests = max_requests
        self.cache_path = cache
        self.cache = {}
        self.use_cache = use_cache
        if use_cache and os.path.exists(cache):
            with open(cache) as f:
                for ln in f:
                    try:
                        d = json.loads(ln)
                        self.cache[d["k"]] = d["r"]
                    except (ValueError, KeyError):
                        pass

    def _redact(self, s):
        return s.replace(self._key, "[REDACTED]") if self._key else s

    def evaluate(self, state, questions):
        body = {"model": MODEL, "state": state, "questions": questions}
        k = hashlib.sha256(json.dumps(body, sort_keys=True).encode()).hexdigest()
        with self.lock:
            if k in self.cache:
                self.cached += 1
                return self.cache[k]
        if self._key is None:
            self._key = _load_key()
        data = json.dumps(body).encode()
        for attempt in range(5):
            with self.lock:
                if self.requests >= self.max_requests:
                    raise RuntimeError("request cap of %d reached" % self.max_requests)
                self.requests += 1
                if attempt:
                    self.retries += 1
            req = urllib.request.Request(ENDPOINT, data=data, method="POST", headers={
                "Authorization": "Bearer " + self._key, "Content-Type": "application/json"})
            try:
                with urllib.request.urlopen(req, timeout=120) as r:
                    resp = json.loads(r.read().decode())
                break
            except urllib.error.HTTPError as e:
                msg = self._redact(e.read().decode(errors="replace"))[:500]
                if e.code in (424, 429, 500, 502, 503, 504) and attempt < 4:  # 424 = upstream 503
                    time.sleep(2 ** attempt * 2)
                    continue
                raise RuntimeError("Jev HTTP %d: %s" % (e.code, msg)) from None
            except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
                if attempt < 4:
                    time.sleep(2 ** attempt * 2)
                    continue
                raise RuntimeError("Jev network error: %s" % self._redact(str(e))) from None
        u = resp.get("usage", {})
        gw = resp.get("providerMetadata", {}).get("gateway", {})
        out = {"answers": resp.get("answers", {}), "usage": u, "cost": gw.get("cost")}
        with self.lock:
            self.input_tokens += u.get("inputTokens", 0)
            self.output_tokens += u.get("outputTokens", 0)
            try:
                self.cost += float(gw.get("cost") or 0)
            except ValueError:
                pass
            self.cache[k] = out
            if self.use_cache:
                os.makedirs(os.path.dirname(self.cache_path), exist_ok=True)
                with open(self.cache_path, "a") as f:
                    f.write(json.dumps({"k": k, "r": out}) + "\n")
        return out

    def summary(self):
        return {"requests": self.requests, "retries": self.retries, "cache_hits": self.cached,
                "input_tokens": self.input_tokens, "output_tokens": self.output_tokens,
                "cost_usd": round(self.cost, 6)}


# ---------------------------------------------------------------- page judging
def has_table(text):
    """Lenient: two or more lines containing a pipe, or an HTML table."""
    return bool(re.search(r"<table", text, re.I)) or sum("|" in ln for ln in text.splitlines()) >= 2


def chunks(text, limit=CHUNK_CHARS):
    if len(text) <= limit:
        return [text]
    out, cur = [], ""
    for para in re.split(r"(\n\s*\n)", text):
        if len(cur) + len(para) > limit and cur.strip():
            out.append(cur)
            cur = ""
        while len(para) > limit:  # a single giant paragraph (e.g. a repetition loop)
            out.append(para[:limit])
            para = para[limit:]
        cur += para
    if cur.strip():
        out.append(cur)
    return out


def page_questions(text):
    qs = {k: v for k, v in QUESTIONS.items() if k != "table_broken" or has_table(text)}
    return qs


def judge_text(jev, text):
    """-> {garbled, interleaved, repetition, table_broken|None, readability (1-5), chunks}."""
    text = re.sub(r"<!--.*?-->", "", text, flags=re.S).strip()
    if not text:
        return {"empty": True, "garbled": None, "interleaved": None, "repetition": None,
                "table_broken": None, "readability": None, "chunks": 0}
    parts = chunks(text)
    res = {q: None for q in BOOL_QS}
    reads = []
    for part in parts:
        a = jev.evaluate(part, page_questions(part))["answers"]
        for q in BOOL_QS:
            if q in a:
                p = a[q].get("probability", a[q].get("noul"))
                res[q] = p if res[q] is None else max(res[q], p)
        if "readability" in a:
            reads.append(a["readability"]["score"] + 1)  # Jev rungs are 0-indexed
    res["readability"] = round(sum(reads) / len(reads), 3) if reads else None
    res["chunks"] = len(parts)
    return res


def judge_doc(jev, md, workers=8):
    pages = split_pages(md)
    nums = sorted(pages)
    with ThreadPoolExecutor(workers) as ex:
        results = list(ex.map(lambda n: judge_text(jev, pages[n][0]), nums))
    return {n: r for n, r in zip(nums, results)}


def summarize(per_page, threshold=None):
    thr = {q: THRESHOLDS[q] if threshold is None else threshold for q in BOOL_QS}
    s = {"pages": len(per_page), "thresholds": thr}
    for q in BOOL_QS:
        threshold = thr[q]
        vals = [r[q] for r in per_page.values() if r.get(q) is not None]
        s[q + "_flag_rate"] = round(sum(v >= threshold for v in vals) / len(vals), 3) if vals else None
        s[q + "_pages_flagged"] = [n for n, r in per_page.items()
                                   if r.get(q) is not None and r[q] >= threshold]
    reads = [r["readability"] for r in per_page.values() if r.get("readability") is not None]
    s["mean_readability"] = round(sum(reads) / len(reads), 3) if reads else None
    s["low_readability_pages"] = [n for n, r in per_page.items()
                                  if r.get("readability") is not None and r["readability"] < READABILITY_FLAG]
    return s


def _fmt(v):
    return "  -  " if v is None else "%5.2f" % v


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("md", nargs="+")
    ap.add_argument("--family", help="document family label (informational)")
    ap.add_argument("--json", help="write full results to this JSON file")
    ap.add_argument("--threshold", type=float, default=None,
                    help="one flag threshold for all boolean questions (default: calibrated per question)")
    ap.add_argument("--engine-compare", action="store_true",
                    help="judge several outputs of the same document side by side")
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--no-cache", action="store_true")
    a = ap.parse_args()
    if len(a.md) > 1 and not a.engine_compare:
        ap.error("several inputs need --engine-compare")
    jev = Jev(use_cache=not a.no_cache)
    docs = {}
    for path in a.md:
        with open(path) as f:
            md = f.read()
        pp = judge_doc(jev, md, a.workers)
        docs[path] = {"family": a.family, "pages": pp, "summary": summarize(pp, a.threshold)}

    cols = ["garbled", "interleaved", "repetition", "table_broken", "readability"]
    short = ["garb", "intl", "rep", "tbl", "read"]
    if a.engine_compare:
        labels = [os.path.relpath(p) for p in a.md]
        for i, l in enumerate(labels):
            print("[%d] %s" % (i, l))
        nums = sorted(set().union(*[d["pages"] for d in docs.values()]))
        print("page  " + "  ".join("%-5s" % s for s in short))
        for n in nums:
            for i, p in enumerate(a.md):
                r = docs[p]["pages"].get(n)
                row = "  ".join(_fmt(r.get(c)) for c in cols) if r else "(missing page)"
                print("%4s [%d] %s" % (n if i == 0 else "", i, row))
        print("mean readability: " + "  ".join("[%d] %s" % (i, docs[p]["summary"]["mean_readability"])
                                                for i, p in enumerate(a.md)))
    else:
        d = docs[a.md[0]]
        print("page  " + "  ".join("%-5s" % s for s in short))
        for n, r in d["pages"].items():
            print("%4d  %s" % (n, "  ".join(_fmt(r.get(c)) for c in cols)))
        s = d["summary"]
        print("flag rate: " + "  ".join("%s@%.2f %s" % (q, s["thresholds"][q], s[q + "_flag_rate"]) for q in BOOL_QS)
              + "  | mean readability %s, pages < %.1f: %s" % (s["mean_readability"], READABILITY_FLAG,
                                                              s["low_readability_pages"]))
    usage = jev.summary()
    print("jev: %(requests)d requests (%(cache_hits)d cached, %(retries)d retries), "
          "%(input_tokens)d in / %(output_tokens)d out tokens, $%(cost_usd).6f" % usage, file=sys.stderr)
    if a.json:
        out = {"threshold": a.threshold, "docs": docs, "usage": usage}
        with open(a.json, "w") as f:
            json.dump(out, f, indent=1)


if __name__ == "__main__":
    main()
