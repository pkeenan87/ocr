"""End-to-end check of the MiniCPM comparison on a synthetic contract.

A fake localhost server stands in for MiniCPM-OCR 1.0 and MiniCPM-OCR Jev. The
baseline is Markdown on disk, not a GPU. Nothing here loads the real weights.

    python eval/minicpm/selftest.py
"""
import json
import shutil
import sys
import tempfile
from pathlib import Path

_HERE = Path(__file__).resolve().parent
_EVAL = _HERE.parent
_ROOT = _EVAL.parent
for _p in (str(_HERE), str(_EVAL)):
    if _p not in sys.path:
        sys.path.insert(0, _p)

import generate  # noqa: E402
import clients  # noqa: E402
import compare  # noqa: E402
import fake_server  # noqa: E402
import reference  # noqa: E402
import regions  # noqa: E402
from fields import canonical, extract  # noqa: E402
from PIL import Image  # noqa: E402


def fail(msg):
    raise SystemExit(f"selftest failed: {msg}")


def test_fields():
    with_initials = canonical("docket", "2:09-cv-00465-LKK-DAD")
    without = canonical("docket", "2:09-cv-00465")
    if with_initials != without:
        fail(f"docket judge initials were not dropped: {with_initials} vs {without}")
    if with_initials[0] != "docket":
        fail(f"docket kind changed: {with_initials}")
    found = extract("See 28 U.S.C. § 1441(a) and ¶ 12 of the agreement.")
    kinds = {k for k, _s in found}
    if "section" not in kinds or "paragraph" not in kinds:
        fail(f"citation extract missed section or paragraph: {found}")
    amount = canonical("amount", "$1,250.50")
    if amount != ("amount", 125050):
        fail(f"amount cents: {amount}")


def test_crop():
    image = Image.new("RGB", (100, 80), "white")
    cropped = regions.crop_image(image, (10, 10, 40, 50))
    if cropped.size != (30, 40):
        fail(f"pixel crop size {cropped.size}")
    norm = regions.crop_image(image, (0.0, 0.0, 0.5, 0.5), "norm")
    if norm.size != (50, 40):
        fail(f"norm crop size {norm.size}")


def test_remote_and_output():
    try:
        clients.require_local("https://example.com/v1")
    except clients.RemoteURL:
        pass
    else:
        fail("remote URL was accepted")
    try:
        compare.assert_output_allowed(_ROOT / "spike" / "heldout" / "nope-results.json")
    except compare.CompareError:
        pass
    else:
        fail("tracked output path was accepted")
    allowed = _ROOT / "eval" / "out" / "minicpm" / "selftest-guard"
    compare.assert_output_allowed(allowed)
    local = _ROOT / "spike" / "heldout-local" / "real.pdf"
    compare.assert_output_allowed(local)


def _export(doc, pages, amount):
    out_pages = []
    for i, text in enumerate(pages, 1):
        values = []
        if amount and amount in text:
            values.append({
                "kind": "amount", "printed": amount, "decision": "correct",
                "readings": {"paddleocr-vl": amount}, "flags": [],
            })
        out_pages.append({
            "doc": doc, "page": i, "values": values, "added": [],
            "text": text, "done": True, "notes": "", "facts": [],
        })
    return {
        "version": 1, "reviewer": "selftest", "exported": "2026-10-27T00:00:00Z",
        "engines": ["paddleocr-vl"], "pages": out_pages,
    }


def _md(pages):
    return "\n".join(f"<!-- page {i} -->\n\n{text}\n" for i, text in enumerate(pages, 1))


def _first_amount(pages):
    import re
    rx = re.compile(r"\$\d[\d,]*\.\d{2}")
    for text in pages:
        for match in rx.finditer(text):
            if match.group(0) not in ("$99,999.00", "$88,888.00"):
                return match.group(0)
    return None


def _replace_all(pages, old, new):
    changed = any(old in text for text in pages)
    return [text.replace(old, new) for text in pages], changed


def test_pairing(tmp):
    scans = tmp / "scans"
    scans.mkdir()
    (scans / "contract.pdf").write_bytes(b"%PDF-1.1\n")
    (scans / "extra.pdf").write_bytes(b"%PDF-1.1\n")
    export = _export("contract", ["The price is $10.00."], "$10.00")
    (scans / "contract.json").write_text(json.dumps(export), encoding="utf-8")
    # Download name the template always uses. Pairing is pages[].doc, and the
    # harness warns because the file is not named extra.json.
    bundle = _export("extra", ["Page one."], None)
    (scans / "holdout_v1.json").write_text(json.dumps(bundle), encoding="utf-8")
    (scans / "orphan.json").write_text(json.dumps(_export("orphan", ["x"], None)), encoding="utf-8")
    prefill = {
        "engines": ["paddleocr-vl"],
        "pages": [{
            "doc": "contract", "page": 1, "transcript_prefill": "ENGINE DRAFT NOT TRUTH",
            "values": [{"kind": "amount", "texts": {"paddleocr-vl": "$1.00"}}],
        }],
    }
    html = "<script>\nconst DATA = " + json.dumps(prefill).replace("</", "<\\/") + ";\n</script>\n"
    (scans / "contract.html").write_text(html, encoding="utf-8")
    embedded = _export("extra", ["Embedded correction."], None)
    both = (
        "<script>const DATA = " + json.dumps(prefill).replace("</", "<\\/")
        + ";\nconst SAVED = " + json.dumps(embedded).replace("</", "<\\/") + ";\n</script>"
    )
    (scans / "extra.html").write_text(both, encoding="utf-8")
    refs, warnings = reference.load_references(
        [scans / "contract.pdf", scans / "extra.pdf", scans / "lonely.pdf"],
        [scans],
    )
    # lonely.pdf is not on disk; pass it explicitly.
    if refs["contract"].path.name != "contract.json":
        fail(f"contract paired with {refs['contract'].path.name}, wanted contract.json")
    if refs["contract"].pages[0].text != "The price is $10.00.":
        fail("contract transcription was not the export text")
    blob = "\n".join(warnings)
    if "transcript_prefill" not in blob:
        fail(f"prefill HTML produced no warning: {warnings}")
    if "ENGINE DRAFT" in blob:
        fail("warning included transcript text")
    if "orphan" not in blob:
        fail(f"orphan correction produced no warning: {warnings}")
    if "lonely" not in blob:
        fail(f"unpaired scan produced no warning: {warnings}")
    if refs["extra"].format != "review-json":
        fail(f"embedded export was not preferred: {refs['extra'].format} {refs['extra'].path.name}")
    # contract.html must not beat contract.json, and its prefill must not become the text.
    if "ENGINE DRAFT" in (refs["contract"].pages[0].text or ""):
        fail("transcript_prefill was used as the correction")


def test_end_to_end():
    work = _ROOT / "eval" / "out" / "minicpm" / "selftest"
    if work.exists():
        shutil.rmtree(work)
    src = work / "src"
    src.mkdir(parents=True)
    generate.write_family("contract", 1, src)
    gt = json.loads((src / "contract.gt.json").read_text(encoding="utf-8"))
    pages = [p["text"] for p in gt["pages"]]
    amount = _first_amount(pages)
    if not amount:
        fail("synthetic contract had no dollar amount")
    base_pages, ok = _replace_all(pages, amount, "$99,999.00")
    mini_pages, ok2 = _replace_all(pages, amount, "$88,888.00")
    if not ok or not ok2:
        fail("could not plant the misread amounts")
    scans = work / "scans"
    scans.mkdir()
    shutil.copy(src / "contract.pdf", scans / "contract.pdf")
    shutil.copy(src / "contract.pdf", scans / "unpaired.pdf")
    (scans / "contract.json").write_text(json.dumps(_export("contract", pages, amount)), encoding="utf-8")
    (scans / "orphan.json").write_text(json.dumps(_export("orphan", ["unused"], None)), encoding="utf-8")
    prefill = {"pages": [{"doc": "unpaired", "page": 1, "transcript_prefill": "not the correction", "values": []}]}
    (scans / "unpaired.html").write_text(
        "<script>const DATA = " + json.dumps(prefill) + ";</script>", encoding="utf-8")
    base_root = work / "baseline-in"
    for name, body in (("contract", base_pages), ("unpaired", pages)):
        dest = base_root / "scans" / f"{name}.md"
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(_md(body), encoding="utf-8")
    server = fake_server.serve(mini_pages)
    host, port = server.server_address
    url = f"http://{host}:{port}"
    try:
        code = compare.main([
            "--input-dir", str(scans),
            "--reference-dir", str(scans),
            "--baseline-md", str(base_root),
            "--minicpm-url", url + "/v1",
            "--jev-url", url,
            "--out", str(work / "run"),
            "--ocr-bin", "missing-ocr-not-used",
        ])
    finally:
        server.shutdown()
    if code != 0:
        fail(f"compare exited {code}")
    report = json.loads((work / "run" / "results.json").read_text(encoding="utf-8"))
    summary = (work / "run" / "summary.md").read_text(encoding="utf-8")
    canary = pages[0][40:80]
    if canary and canary in summary:
        fail("summary.md contains page text")
    for needle in ("99,999.00", "88,888.00", amount):
        if needle in summary:
            fail(f"summary.md contains a field value ({needle})")
    base = report["engines"]["baseline"]
    mini = report["engines"]["minicpm"]
    if not base["accuracy"]["len_sum"] or base["accuracy"]["dist_sum"] <= 0:
        fail(f"baseline CER was not positive: {base['accuracy']}")
    if not mini["accuracy"]["len_sum"] or mini["accuracy"]["dist_sum"] <= 0:
        fail(f"minicpm CER was not positive: {mini['accuracy']}")
    if (base["fields"] or {}).get("recall") is None or base["fields"]["recall"] >= 1:
        fail(f"baseline field recall: {base.get('fields')}")
    if (mini["fields"] or {}).get("recall") is None or mini["fields"]["recall"] >= 1:
        fail(f"minicpm field recall: {mini.get('fields')}")
    if not mini["speed"]["pages_per_s"]:
        fail(f"minicpm speed missing: {mini['speed']}")
    for key in ("baseline", "minicpm"):
        jev = report["jev"][key]
        if jev["auroc"] is None or jev["auroc"] < 0.9:
            fail(f"{key} Jev AUROC {jev.get('auroc')}")
        row = next(r for r in jev["thresholds"] if r["threshold"] == 0.5)
        if row["critical_errors_caught"] != 1.0:
            fail(f"{key} critical catch at 0.5 was {row['critical_errors_caught']}")
        if "full_page" not in jev["image_scope"]:
            fail(f"{key} image scope {jev['image_scope']}")
    blob = "\n".join(report["warnings"])
    if "unpaired" not in blob or "orphan" not in blob or "transcript_prefill" not in blob:
        fail(f"missing unpaired warning: {report['warnings']}")


def test_protocol_image():
    tmp = Path(tempfile.mkdtemp())
    image = tmp / "one.png"
    Image.new("RGB", (32, 32), "white").save(image)
    server = fake_server.serve(["recognized line"])
    host, port = server.server_address
    url = f"http://{host}:{port}"
    try:
        text, _records, _wall = clients.recognize_pages(image, url + "/v1", "minicpm-ocr", 72, 32, "llama", 30)
        if "recognized line" not in text:
            fail(f"chat text missing: {text!r}")
        clients.score_regions(url, [{
            "case_id": "a",
            "image": "data:image/png;base64,aaaa",
            "ocr_text": "Total $99,999.00",
            "category": "text",
        }], 30, "rm")
        flags = clients._request(url + "/__control")
    finally:
        server.shutdown()
        shutil.rmtree(tmp)
    if flags["image_before_text"] != 1 or flags["bad_prompt"]:
        fail(f"prompt shape: {flags}")
    if flags["ternary"] != 1:
        fail(f"ternary flag: {flags}")
    if flags["chat_calls"] != 1 or flags["score_calls"] != 1:
        fail(f"call counts: {flags}")


def main():
    test_fields()
    test_crop()
    test_remote_and_output()
    with tempfile.TemporaryDirectory() as tmp:
        test_pairing(Path(tmp))
    test_protocol_image()
    test_end_to_end()
    print("EXERCISED: review-export and review-HTML pairing, unpaired warnings,")
    print("  CER/WER and critical-field precision/recall, disagreement path,")
    print("  Jev AUROC / precision / recall / critical-error catch on planted misreads,")
    print("  localhost refusal, gitignored-output refusal, page render, crop,")
    print("  image-then-Text-Recognition prompt, /v1/rm/score input_form=ternary.")
    print("MOCKED: MiniCPM-OCR 1.0 weights, MiniCPM-OCR Jev weights, paddleocr-vl,")
    print("  PP-DocLayoutV3 crops, the vendor Chromium/LaTeX renderer, VRAM and pages/s on a GPU.")
    print("selftest ok")


if __name__ == "__main__":
    main()
