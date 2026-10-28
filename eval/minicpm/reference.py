"""Hand corrections that pair with a scan by filename stem.

The editor is ``eval/holdout/review_template.html``. ``eval/holdout/prepare.py build``
fills ``const DATA = /*DATA*/null`` with one JSON object (``</`` written as ``<\\/``).
That object is the shell the reviewer opens. ``pages[].transcript_prefill`` is an
engine transcription, not the correction.

The correction is the Export JSON the page downloads. The download name is always
``holdout_v1.json``. ``eval/holdout/score.py`` joins ``pages[].doc`` to the PDF stem
(``h01`` ↔ ``h01.pdf``). This loader does the same.

Put the correction next to the scan, or in any ``--reference-dir``:

- ``contract.pdf`` pairs with ``contract.json`` (rename the download).
  Every page needs ``"doc": "contract"``.
- ``contract.html`` is also read: a review page whose ``const DATA`` is the export,
  or an HTML file that embeds the export. A DATA shell that only has
  ``transcript_prefill`` is not a correction.
- ``contract.md``, ``contract.txt`` and ``contract.gt.json`` (from ``eval/generate.py``)
  are transcriptions. ``<!-- page N -->`` splits pages.
- One export may cover several scans. Pairing then uses ``pages[].doc``, and the
  harness warns that the file was not named ``<stem>.json``.

Example pair: ``spike/heldout-local/contract.pdf`` and
``spike/heldout-local/contract.json``.
"""
import json
import sys
from collections import Counter
from pathlib import Path

_HERE = Path(__file__).resolve().parent
_EVAL = _HERE.parent
for _p in (str(_HERE), str(_EVAL)):
    if _p not in sys.path:
        sys.path.insert(0, _p)

import score as scorer  # noqa: E402
from fields import canonical  # noqa: E402

_spec_path = _EVAL / "holdout" / "score.py"
import importlib.util  # noqa: E402

_spec = importlib.util.spec_from_file_location("holdout_score_ref", _spec_path)
_holdout = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_holdout)

SCAN_EXTS = {".pdf", ".png", ".jpg", ".jpeg", ".tif", ".tiff", ".webp", ".gif", ".bmp"}
# Kinds the review template offers that this harness treats as critical.
_REVIEW_CRITICAL = {"amount", "date", "docket", "citation", "paragraph", "section"}
EXAMPLE_PAIR = (
    "spike/heldout-local/contract.pdf with spike/heldout-local/contract.json "
    '(Export JSON; every page has "doc": "contract"). '
    "contract.html is also accepted when it embeds that export."
)


class PageRef:
    def __init__(self, page, text, fields, furniture=None, tokens=None, bates=None, kind=None):
        self.page = page
        self.text = text  # None when the page has no transcription
        self.fields = fields  # Counter, or None to extract from text
        self.furniture = furniture or []
        self.tokens = tokens or []
        self.bates = bates
        self.kind = kind


class DocRef:
    def __init__(self, stem, path, fmt, pages):
        self.stem = stem
        self.path = path
        self.format = fmt
        self.pages = pages

    def field_counters(self):
        out = []
        for page in self.pages:
            if page.fields is not None:
                out.append(page.fields)
            elif page.text is not None:
                from fields import counter
                out.append(counter(page.text))
            else:
                out.append(Counter())
        return out

    def has_text(self):
        return bool(self.pages) and all(p.text is not None for p in self.pages)

    def gt(self):
        return {"pages": [{
            "text": p.text or "",
            "furniture": p.furniture,
            "tokens": p.tokens,
            "bates": p.bates,
            "kind": p.kind,
        } for p in self.pages]}


def iter_scans(dirs):
    found = []
    for d in dirs:
        root = Path(d)
        if not root.exists():
            continue
        for path in sorted(root.rglob("*")):
            if path.is_file() and path.suffix.lower() in SCAN_EXTS and not path.name.startswith("."):
                found.append(path)
    return found


def _stem(path):
    name = path.name
    if name.endswith(".gt.json"):
        return name[: -len(".gt.json")]
    return path.stem


def _balanced(text, start):
    opener = text[start]
    closer = "}" if opener == "{" else "]"
    depth = 0
    i = start
    n = len(text)
    in_str = False
    esc = False
    while i < n:
        ch = text[i]
        if in_str:
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
        else:
            if ch == '"':
                in_str = True
            elif ch == opener:
                depth += 1
            elif ch == closer:
                depth -= 1
                if depth == 0:
                    return text[start:i + 1]
        i += 1
    return None


def iter_json_values(text):
    i = 0
    while True:
        j = text.find("{", i)
        k = text.find("[", i)
        if j < 0 and k < 0:
            return
        start = j if k < 0 or (j >= 0 and j < k) else k
        blob = _balanced(text, start)
        if not blob:
            i = start + 1
            continue
        try:
            yield json.loads(blob)
        except json.JSONDecodeError:
            pass
        i = start + 1


def _is_export(obj):
    return isinstance(obj, dict) and obj.get("version") == 1 and isinstance(obj.get("pages"), list)


def _is_gt(obj):
    pages = obj.get("pages") if isinstance(obj, dict) else None
    if not isinstance(pages, list) or not pages or not isinstance(pages[0], dict):
        return False
    return "tokens" in pages[0] or "furniture" in pages[0] or "family" in obj


def _is_data(obj):
    pages = obj.get("pages") if isinstance(obj, dict) else None
    if not isinstance(pages, list) or not pages or not isinstance(pages[0], dict):
        return False
    sample = pages[0]
    return "transcript_prefill" in sample or "values" in sample or "doc" in sample


def _corrected(page):
    vals = list(page.get("values") or []) + list(page.get("added") or [])
    if any(v.get("decision") for v in vals):
        return True
    text = page.get("text")
    return isinstance(text, str) and bool(text.strip())


def _review_counter(page):
    """Canonical fields the reviewer marked correct. None when nothing was decided."""
    vals = list(page.get("values") or []) + list(page.get("added") or [])
    if not any(v.get("decision") for v in vals):
        return None
    counts = Counter()
    for v in vals:
        if v.get("decision") != "correct":
            continue
        printed = (v.get("printed") or "").strip()
        kind = v.get("kind")
        if not printed or kind not in _REVIEW_CRITICAL:
            continue
        if kind in ("amount", "date", "docket"):
            counts[_holdout.typed(kind, printed)] += 1
        else:
            counts[canonical(kind, printed)] += 1
    return counts


def _pages_from_export(pages):
    out = []
    for i, page in enumerate(pages, 1):
        number = int(page.get("page") or i)
        text = page.get("text") if isinstance(page.get("text"), str) else None
        out.append(PageRef(number, text, _review_counter(page)))
    out.sort(key=lambda p: p.page)
    return out


def _pages_from_gt(obj):
    out = []
    for i, page in enumerate(obj["pages"], 1):
        out.append(PageRef(
            i, page.get("text") or "", None,
            furniture=page.get("furniture") or [],
            tokens=page.get("tokens") or [],
            bates=page.get("bates"),
            kind=page.get("kind"),
        ))
    return out


def _pages_from_text(text):
    parts = scorer.split_pages(text)
    if not parts:
        return [PageRef(1, "", None)]
    last = max(parts)
    return [PageRef(i, parts.get(i, ("", ""))[0], None) for i in range(1, last + 1)]


def _read_payloads(path):
    raw = path.read_text(encoding="utf-8")
    if path.suffix.lower() == ".json" or path.name.endswith(".gt.json"):
        try:
            return [json.loads(raw)]
        except json.JSONDecodeError:
            return []
    return list(iter_json_values(raw))


def _groups_from_obj(obj, path):
    """``(fmt, {doc: pages}, warnings)`` or None. Warnings apply only if the caller keeps this object."""
    warnings = []
    if _is_export(obj):
        groups = {}
        for page in obj["pages"]:
            if not isinstance(page, dict):
                continue
            doc = str(page.get("doc") or _stem(path))
            groups.setdefault(doc, []).append(page)
        built = {doc: _pages_from_export(pages) for doc, pages in groups.items()}
        docs = sorted(groups)
        if len(docs) == 1 and docs[0] != _stem(path):
            warnings.append(
                f"{path}: pages[].doc is {docs[0]!r} but the filename stem is {_stem(path)!r}. "
                f"Pairing uses the filename stem when the file is named <stem>.json, and pages[].doc otherwise. "
                f"Rename it so they match. Example: {EXAMPLE_PAIR}"
            )
        elif len(docs) > 1 and _stem(path) not in docs:
            warnings.append(
                f"{path}: one export covers {', '.join(docs)}. Each scan is paired by pages[].doc. "
                f"Prefer one file per scan named <stem>.json. Example: {EXAMPLE_PAIR}"
            )
        return "review-json", built, warnings
    if _is_gt(obj) and not _is_export(obj):
        return "gt-json", {_stem(path): _pages_from_gt(obj)}, warnings
    if _is_data(obj):
        pages = [p for p in obj["pages"] if isinstance(p, dict)]
        if not any(_corrected(p) for p in pages):
            warnings.append(
                f"{path}: review HTML has no corrected text and no value decision. "
                "transcript_prefill is the engine draft, not a hand correction. "
                f"Export JSON from the template and save it as {_stem(path)}.json. Example: {EXAMPLE_PAIR}"
            )
            return "review-html", {}, warnings
        groups = {}
        for page in pages:
            if not _corrected(page):
                warnings.append(
                    f"{path}: page {page.get('page')} of doc {page.get('doc')!r} has no correction; "
                    "transcript_prefill was not used"
                )
                continue
            doc = str(page.get("doc") or _stem(path))
            groups.setdefault(doc, []).append(page)
        if not groups:
            return "review-html", {}, warnings
        return "review-html", {doc: _pages_from_export(ps) for doc, ps in groups.items()}, warnings
    return None


def load_references(scan_paths, reference_dirs):
    """``{stem: DocRef}`` and warning strings. Scans are identified by filename stem."""
    warnings = []
    stems = {}
    for path in scan_paths:
        stems.setdefault(path.stem, []).append(path)
    for stem, paths in stems.items():
        if len(paths) > 1:
            warnings.append(
                "more than one scan is named "
                + stem
                + ": "
                + ", ".join(str(p) for p in paths)
                + ". A correction named "
                + stem
                + ".json pairs with all of them; give the scans distinct stems."
            )

    # A file named <stem>.json beats a multi-doc export, and an export beats a
    # DATA shell or a plain transcription of the same stem.
    rank = {"review-json": 0, "gt-json": 1, "review-html": 2, "text": 3}
    by_stem = {}
    by_doc = {}
    seen_docs = []  # (doc, path) for unpaired-reference warnings

    def _prefer(slot, key, ref):
        old = slot.get(key)
        if old is None or rank.get(ref.format, 9) < rank.get(old.format, 9):
            slot[key] = ref

    dirs = [Path(d) for d in reference_dirs if Path(d).exists()]
    missing = [str(d) for d in reference_dirs if not Path(d).exists()]
    for d in missing:
        warnings.append(f"reference dir does not exist yet: {d}")

    ref_files = []
    for root in dirs:
        for path in sorted(root.rglob("*")):
            if not path.is_file() or path.name.startswith("."):
                continue
            suf = path.suffix.lower()
            if path.name.endswith(".gt.json") or suf in {".json", ".html", ".md", ".txt"}:
                if path.name.endswith(".timing.json"):
                    continue
                ref_files.append(path)

    for path in ref_files:
        if path.suffix.lower() in {".md", ".txt"} and not path.name.endswith(".json"):
            # A markdown/text file is a transcription only when its stem matches a scan,
            # so an unrelated note in the folder is not treated as ground truth.
            if path.stem not in stems:
                continue
            doc = DocRef(path.stem, path, "text", _pages_from_text(path.read_text(encoding="utf-8")))
            _prefer(by_stem, path.stem, doc)
            seen_docs.append((path.stem, path))
            continue
        try:
            payloads = _read_payloads(path)
        except OSError as exc:
            warnings.append(f"{path}: could not read ({exc})")
            continue
        if not payloads and path.suffix.lower() in {".json", ".html"}:
            if path.stem in stems or path.name.endswith(".json"):
                warnings.append(f"{path}: no review export or ground truth found in the file")
            continue
        chosen = None
        for obj in payloads:
            if not isinstance(obj, dict):
                continue
            got = _groups_from_obj(obj, path)
            if got is None:
                continue
            fmt, groups, obj_warnings = got
            # An embedded export beats a DATA shell in the same HTML file.
            # An empty group is a shell with nothing but transcript_prefill.
            if not groups and fmt != "review-json":
                if chosen is None:
                    chosen = (fmt, groups, obj_warnings)
                continue
            if chosen is None or chosen[1] == {} or (fmt == "review-json" and chosen[0] != "review-json"):
                chosen = (fmt, groups, obj_warnings)
            if fmt == "review-json" and groups:
                break
        if chosen is None:
            continue
        fmt, groups, obj_warnings = chosen
        warnings.extend(obj_warnings)
        if not groups:
            continue
        for doc, pages in groups.items():
            seen_docs.append((doc, path))
            ref = DocRef(doc, path, fmt, pages)
            if path.stem == doc or (path.name.endswith(".gt.json") and _stem(path) == doc):
                _prefer(by_stem, doc, ref)
            _prefer(by_doc, doc, ref)

    paired = {}
    used_paths = set()
    for stem in stems:
        ref = by_stem.get(stem) or by_doc.get(stem)
        if ref is None:
            warnings.append(
                f"{stem}: scan has no correction. Save the template export as {stem}.json "
                f'(pages[].doc = "{stem}") next to the scan or in the reference dir. Example: {EXAMPLE_PAIR}'
            )
            continue
        paired[stem] = ref
        used_paths.add(Path(ref.path).resolve())
        docs_in_file = {d for d, p in seen_docs if Path(p).resolve() == Path(ref.path).resolve()}
        if stem not in docs_in_file and ref.format.startswith("review"):
            warnings.append(
                f"{ref.path}: paired with {stem} by filename, but pages[].doc is "
                + ", ".join(sorted(docs_in_file))
                + f'. Set "doc" to "{stem}".'
            )

    for doc, path in seen_docs:
        if doc in stems:
            continue
        if Path(path).resolve() in used_paths and doc not in stems:
            warnings.append(
                f"{path}: correction doc {doc!r} has no scan named {doc}.pdf "
                "(or another supported image). The harness pairs on the filename stem and pages[].doc."
            )
        elif Path(path).resolve() not in used_paths:
            warnings.append(
                f"{path}: correction doc {doc!r} has no matching scan. Example pair: {EXAMPLE_PAIR}"
            )

    # De-duplicate while keeping order.
    uniq = []
    seen = set()
    for w in warnings:
        if w not in seen:
            seen.add(w)
            uniq.append(w)
    return paired, uniq
