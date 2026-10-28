"""Compare paddleocr-vl (or another explicit engine) with MiniCPM-OCR 1.0, then MiniCPM-OCR Jev.

This does not change `ocr`'s default engine. The baseline is whatever `--baseline-model`
names (paddleocr-vl on Linux). MiniCPM is an eval-only client of a localhost server.

Real-document output defaults to ``eval/out/minicpm/``, which is gitignored. The run
refuses to write inside the repo unless `git check-ignore` says the path is ignored.

    python eval/minicpm/compare.py --input-dir spike/heldout --input-dir spike/heldout-local \\
        --reference-dir spike/heldout-local --only signature_block.pdf \\
        --use-real-config --out eval/out/minicpm/smoke
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

_HERE = Path(__file__).resolve().parent
_EVAL = _HERE.parent
_ROOT = _EVAL.parent
for _p in (str(_HERE), str(_EVAL)):
    if _p not in sys.path:
        sys.path.insert(0, _p)

import score as scorer  # noqa: E402
import clients  # noqa: E402
import metrics  # noqa: E402
import reference  # noqa: E402
import regions  # noqa: E402
from fields import counter as field_counter  # noqa: E402

THRESHOLDS = (0.5, 0.8, 0.9, 0.95)
HEADLINE = ("amount", "date", "docket")
JEV_BATCH = 16


class CompareError(SystemExit):
    pass


def git_root():
    for p in [_ROOT, *_ROOT.parents]:
        if (p / ".git").exists():
            return p
    return None


def assert_output_allowed(path):
    """Real OCR text must not land in a tracked path. Fail closed if git is missing."""
    path = Path(path).resolve()
    repo = git_root()
    if repo is None:
        raise CompareError(
            "refusing to write results: git is not available, so the harness cannot check "
            "that the output path is ignored"
        )
    try:
        rel = path.relative_to(repo)
    except ValueError:
        return path
    proc = subprocess.run(
        ["git", "-C", str(repo), "check-ignore", "-q", "--", rel.as_posix()],
        capture_output=True,
    )
    if proc.returncode != 0:
        raise CompareError(
            f"refusing to write {path}: it is inside the repo and not gitignored. "
            "Use eval/out/minicpm/ so page text cannot be committed."
        )
    return path


def _rel_tag(scan, input_dirs):
    resolved = scan.resolve()
    for d in input_dirs:
        root = Path(d).resolve()
        try:
            rel = resolved.relative_to(root)
        except ValueError:
            continue
        return Path(root.name) / rel
    return Path(scan.name)


def _md_path(root, tag):
    return Path(root) / tag.with_suffix(".md")


def _find_saved(root, tag):
    if not root:
        return None
    root = Path(root)
    if root.is_file():
        return root
    cand = _md_path(root, tag)
    if cand.exists():
        return cand
    flat = root / f"{tag.stem}.md"
    if flat.exists():
        return flat
    return None


def _page_bodies(md):
    parts = scorer.split_pages(md or "")
    if not parts:
        return []
    last = max(parts)
    return [parts.get(i, ("", ""))[0] for i in range(1, last + 1)]


def _mean(xs):
    xs = [x for x in xs if x is not None]
    return round(sum(xs) / len(xs), 4) if xs else None


def _speed(pages, wall, per_page):
    per_page = [x for x in per_page if x is not None]
    return {
        "pages": pages,
        "wall_s": None if wall is None else round(wall, 4),
        "pages_per_s": round(pages / wall, 4) if wall else None,
        "per_page_s": [round(x, 4) for x in per_page],
        "per_page_s_mean": _mean(per_page),
    }


def _timing_file(md_path):
    md_path = Path(md_path)
    return md_path.with_name(md_path.stem + ".timing.json")


def _speed_from_timing(path):
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    pages = data.get("pages") or []
    times = [p.get("recognize_s") for p in pages]
    wall = (data.get("totals") or {}).get("wall_s")
    return _speed(len(pages), wall, times)


def _isolated_env(out, use_real):
    env = dict(os.environ)
    if use_real:
        return env
    config = out / "state" / "config"
    state = out / "state" / "state"
    config.mkdir(parents=True, exist_ok=True)
    state.mkdir(parents=True, exist_ok=True)
    env["OCR_CONFIG_DIR"] = str(config)
    env["OCR_STATE_DIR"] = str(state)
    return env


def _run_baseline(scan, md_path, model, dpi, ocr_bin, env):
    md_path.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        ocr_bin, "scan",
        "--input-file", str(scan),
        "--output-file", str(md_path),
        "--model", model,
        "--overwrite", "--quiet", "--no-front-matter", "--timing-json",
    ]
    if dpi is not None:
        cmd += ["--dpi", str(dpi)]
    proc = subprocess.run(cmd, env=env, capture_output=True, text=True)
    if proc.returncode not in (0, 1) or not md_path.exists():
        tail = (proc.stderr or proc.stdout or "")[-400:].strip()
        raise RuntimeError(f"ocr scan exited {proc.returncode}: {tail}")
    return proc.returncode


def _public_region(region):
    keep = (
        "page", "region", "category", "label", "error", "cer", "critical_errors",
        "verdict", "prob_yes", "image_scope",
    )
    return {k: region.get(k) for k in keep}


def _score_text(ref, hyp_md):
    if ref is None or not ref.has_text():
        return None
    try:
        scored = metrics.score_markdown(ref.gt(), hyp_md)
    except scorer.PageError as exc:
        return {"error": str(exc)}
    keep = ("cer", "wer", "cer_strict", "wer_strict", "dist_sum", "len_sum", "wdist", "wlen", "pages", "pages_in_output")
    return {k: scored.get(k) for k in keep}


def _score_fields(ref, bodies):
    if ref is None:
        return None
    hyp = [field_counter(t) for t in bodies]
    # Zip to the reference page count so a short hypothesis leaves the rest as misses.
    n = max(len(ref.pages), len(hyp))
    ref_c = ref.field_counters()
    while len(ref_c) < n:
        ref_c.append(Counter())
    while len(hyp) < n:
        hyp.append(Counter())
    report = metrics.field_scores_from_counters(ref_c, hyp)
    return metrics.public_fields(report)


def _jev_on_pages(scan, bodies, ref, url, protocol, dpi, timeout, layout_dir):
    images = {}
    try:
        for number, image in clients.render_document(scan, dpi):
            images[number] = image
    except (OSError, RuntimeError, ValueError) as exc:
        return {"error": str(exc)[:300], "regions": [], "dropped_critical": []}
    labeled = []
    dropped = []
    pending = []
    for index, text in enumerate(bodies, 1):
        ref_text = None
        if ref is not None:
            match = next((p for p in ref.pages if p.page == index), None)
            if match is None and index <= len(ref.pages):
                match = ref.pages[index - 1]
            ref_text = None if match is None else match.text
        layout = None
        if layout_dir:
            layout = regions.load_layout(Path(layout_dir) / f"{scan.stem}.json", index)
        page_regions, page_dropped = regions.label_page(ref_text, text, layout)
        image = images.get(index)
        for region in page_regions:
            region["page"] = index
            bbox = region.get("bbox")
            if image is None:
                region["image_scope"] = "missing"
                region["verdict"] = "missing_original_image"
                region["prob_yes"] = 0.0
                continue
            if bbox:
                coord = "px"
                if layout:
                    coord = next((b.get("coord") or "px" for b in layout if b.get("bbox") == bbox), "px")
                crop = regions.crop_image(image, bbox, coord)
                region["image_scope"] = "crop"
            else:
                crop = image
                region["image_scope"] = "full_page"
            item = {
                "case_id": f"{scan.stem}-p{index}-r{region['region']}",
                "image": clients.data_url(clients.png_bytes(crop)),
                "ocr_text": region.get("text") or "",
                "category": region.get("category") or "text",
            }
            if protocol == "score":
                item["render_image"] = clients.data_url(clients.render_candidate(item["ocr_text"]))
            pending.append((region, item))
            labeled.append(region)
        for item in page_dropped:
            item["page"] = index
            dropped.append(item)
    for start in range(0, len(pending), JEV_BATCH):
        batch = pending[start:start + JEV_BATCH]
        try:
            results = clients.score_regions(url, [item for _, item in batch], timeout, protocol)
        except (RuntimeError, OSError, ValueError) as exc:
            for region, _item in batch:
                region["verdict"] = "forward_error"
                region["prob_yes"] = -1.0
                region["detail"] = str(exc)[:200]
            continue
        for (region, _item), result in zip(batch, results):
            if protocol == "score":
                if result.get("error"):
                    region["verdict"] = "forward_error"
                    region["prob_yes"] = -1.0
                else:
                    region["verdict"] = "scored"
                    region["prob_yes"] = result.get("prob_1")
            else:
                region["verdict"] = result.get("verdict")
                region["prob_yes"] = result.get("prob_yes")
        if len(results) < len(batch):
            for region, _item in batch[len(results):]:
                region["verdict"] = "forward_error"
                region["prob_yes"] = -1.0
    return {"regions": labeled, "dropped_critical": dropped}


def _strip_text(jev):
    if not jev or jev.get("error"):
        return jev
    return {
        "regions": [_public_region(r) for r in jev.get("regions") or []],
        "dropped_critical": jev.get("dropped_critical") or [],
        "report": metrics.jev_report(jev.get("regions") or [], jev.get("dropped_critical") or [], THRESHOLDS),
    }


def _accumulate(bucket, part):
    if not part:
        return
    for key in ("dist_sum", "len_sum", "wdist", "wlen"):
        if part.get(key) is not None:
            bucket[key] = bucket.get(key, 0) + part[key]
    fields = part.get("fields")
    if fields:
        bucket.setdefault("field_rows", []).append(fields)
    speed = part.get("speed")
    if speed:
        bucket.setdefault("speeds", []).append(speed)


def _merge_fields(rows):
    if not rows:
        return None
    ref_c, hyp_c = Counter(), Counter()
    # public_fields dropped the counters. Recompute from tp/ref/hyp is not enough per kind.
    # The file entry keeps by_kind tp/ref/hyp, so merge those.
    by = {}
    tp = ref = hyp = 0
    for row in rows:
        tp += row.get("tp") or 0
        ref += row.get("ref") or 0
        hyp += row.get("hyp") or 0
        for kind, cell in (row.get("by_kind") or {}).items():
            slot = by.setdefault(kind, {"tp": 0, "ref": 0, "hyp": 0})
            slot["tp"] += cell.get("tp") or 0
            slot["ref"] += cell.get("ref") or 0
            slot["hyp"] += cell.get("hyp") or 0
    def ratio(hit, base):
        return round(hit / base, 4) if base else None
    return {
        "tp": tp, "ref": ref, "hyp": hyp,
        "precision": ratio(tp, hyp), "recall": ratio(tp, ref),
        "by_kind": {
            k: {**v, "precision": ratio(v["tp"], v["hyp"]), "recall": ratio(v["tp"], v["ref"])}
            for k, v in sorted(by.items())
        },
    }


def _merge_speed(speeds):
    pages = sum(s.get("pages") or 0 for s in speeds)
    wall = sum(s["wall_s"] for s in speeds if s.get("wall_s") is not None)
    times = [t for s in speeds for t in (s.get("per_page_s") or [])]
    have_wall = any(s.get("wall_s") is not None for s in speeds)
    return _speed(pages, wall if have_wall else None, times)


def _merge_jev(parts):
    regions_ = []
    dropped = []
    for part in parts:
        if not part or part.get("error"):
            continue
        regions_.extend(part.get("regions") or [])
        dropped.extend(part.get("dropped_critical") or [])
    if not regions_ and not dropped:
        return None
    return metrics.jev_report(regions_, dropped, THRESHOLDS)


def _fmt_rate(value):
    return "n/a" if value is None else f"{value:.4f}"


def _kind_cell(fields, kind):
    if not fields:
        return "n/a"
    cell = (fields.get("by_kind") or {}).get(kind)
    if not cell or (cell.get("ref") == 0 and cell.get("hyp") == 0):
        return "n/a"
    return (
        f"{_fmt_rate(cell.get('precision'))} / {_fmt_rate(cell.get('recall'))} "
        f"(ref {cell.get('ref')}, hyp {cell.get('hyp')})"
    )


def _engine_block(name, acc, speed, fields):
    cer = (acc.get("dist_sum", 0) / acc["len_sum"]) if acc.get("len_sum") else None
    wer = (acc.get("wdist", 0) / acc["wlen"]) if acc.get("wlen") else None
    lines = [
        f"### {name}",
        "",
        f"- CER {_fmt_rate(None if not acc.get('len_sum') else round(cer, 4))}",
        f"- WER {_fmt_rate(None if not acc.get('wlen') else round(wer, 4))}",
        f"- critical fields precision / recall {_fmt_rate((fields or {}).get('precision'))} / {_fmt_rate((fields or {}).get('recall'))}",
    ]
    for kind in HEADLINE:
        lines.append(f"- {kind} precision / recall {_kind_cell(fields, kind)}")
    extra = [k for k in (fields or {}).get("by_kind", {}) if k not in HEADLINE]
    for kind in extra:
        lines.append(f"- {kind} precision / recall {_kind_cell(fields, kind)}")
    if speed:
        wall = "n/a" if speed.get("wall_s") is None else f"{speed['wall_s']}s"
        mean = "n/a" if speed.get("per_page_s_mean") is None else f"{speed['per_page_s_mean']}s"
        lines.append(
            f"- pages/s {_fmt_rate(speed.get('pages_per_s'))} "
            f"over {speed.get('pages')} pages, wall {wall}, mean per-page {mean}"
        )
    lines.append("")
    return lines


def render_summary(report):
    """Counts and rates only. No page text and no field values."""
    lines = [
        "# MiniCPM-OCR comparison",
        "",
        "MiniCPM-OCR Jev is the local OpenBMB region scorer. It is not the TypeSafe judge in `eval/judge.py`.",
        "",
        f"Baseline engine: `{report['baseline_model']}`. MiniCPM mode: `{report['minicpm_mode']}` ({report['minicpm_backend']}).",
        f"Files: {report['files_total']}. With a correction: {report['files_with_reference']}. Without: {report['files_without_reference']}.",
        f"Warnings: {len(report['warnings'])}.",
        "",
        "Rates below are micro-averages over files that have a correction. Files without one contribute speed and, when both engines ran, disagreement counts.",
        "",
        "## Accuracy and speed",
        "",
    ]
    for name, key in (("Baseline", "baseline"), ("MiniCPM-OCR 1.0", "minicpm")):
        block = report["engines"].get(key) or {}
        if report.get("skipped", {}).get(key):
            lines.append(f"### {name}")
            lines.append("")
            lines.append("Skipped.")
            lines.append("")
            continue
        lines.extend(_engine_block(name, block.get("accuracy") or {}, block.get("speed"), block.get("fields")))
    lines.append("## Disagreement on files without a correction")
    lines.append("")
    dis = report.get("disagreement")
    if not dis:
        lines.append("No engine-vs-engine disagreement was computed.")
    else:
        lines.append("These counts are not accuracy. They are critical fields one engine extracted and the other did not.")
        lines.append("")
        lines.append(
            f"Critical fields only in the baseline: {dis['only_baseline']}. "
            f"Only in MiniCPM: {dis['only_minicpm']}. Shared: {dis['shared']}."
        )
        for kind, cell in (dis.get("by_kind") or {}).items():
            lines.append(
                f"- {kind}: baseline {cell['only_baseline']}, MiniCPM {cell['only_minicpm']}, shared {cell['shared']}"
            )
    lines.append("")
    lines.append("## MiniCPM-OCR Jev")
    lines.append("")
    lines.append(
        "A review flag is `verdict == scored` and `prob_yes` below the threshold. "
        "AUROC uses `1 - prob_yes` as the error score. "
        "`critical_errors_caught` is the share of critical-field mismatches (amounts, dates, "
        "dockets, citations, sections, paragraphs) that sit on a flagged region. "
        "A reference field whose block was not paired counts as not caught."
    )
    lines.append("")
    for name, key in (("baseline output", "baseline"), ("MiniCPM output", "minicpm")):
        jev = (report.get("jev") or {}).get(key)
        lines.append(f"### {name}")
        lines.append("")
        if not jev:
            lines.append("Not run.")
            lines.append("")
            continue
        lines.append(
            f"- regions {jev['regions']}, labeled {jev['labeled']}, errors {jev['errors']}, "
            f"clean {jev['clean']}, unscored {jev['unscored']}"
        )
        lines.append(f"- AUROC {_fmt_rate(jev.get('auroc'))}")
        lines.append(f"- critical-field errors {jev['critical_errors']} (unpaired reference fields {jev['dropped_critical_unpaired']})")
        lines.append(f"- image scope: {', '.join(jev.get('image_scope') or [])}")
        if "full_page" in (jev.get("image_scope") or []):
            lines.append(
                "- Full-page images are not the crops Jev was trained on. Treat this AUROC as a degraded setup unless a layout file supplied boxes."
            )
        lines.append("")
        lines.append("| threshold | precision | recall | critical-field errors caught |")
        lines.append("| --- | --- | --- | --- |")
        for row in jev.get("thresholds") or []:
            lines.append(
                f"| {row['threshold']} | {_fmt_rate(row.get('precision'))} | {_fmt_rate(row.get('recall'))} | {_fmt_rate(row.get('critical_errors_caught'))} |"
            )
        lines.append("")
    if report["warnings"]:
        lines.append("## Warnings")
        lines.append("")
        for w in report["warnings"]:
            lines.append(f"- {w}")
        lines.append("")
    lines.append("## Where this came from")
    lines.append("")
    lines.append("`results.json` beside this file has the same rates plus per-file numbers. It is gitignored. It may contain canonical field values. Do not commit it, and do not paste page text anywhere.")
    lines.append("")
    return "\n".join(lines)


def build_parser():
    p = argparse.ArgumentParser(description="Compare the current OCR engine with MiniCPM-OCR 1.0 and score MiniCPM-OCR Jev.")
    p.add_argument("--input-dir", action="append", required=True, help="directory of PDFs and images; repeat for spike/heldout and spike/heldout-local")
    p.add_argument("--reference-dir", action="append", help="corrections. Defaults to every --input-dir. Example: spike/heldout-local")
    p.add_argument("--out", default="eval/out/minicpm/run1")
    p.add_argument("--baseline-model", default="paddleocr-vl", help="passed to ocr scan --model. Does not change the binary default.")
    p.add_argument("--baseline-md", help="directory of Markdown already produced for the baseline, laid out as <out>/baseline/<dir>/<stem>.md")
    p.add_argument("--minicpm-url", default="http://127.0.0.1:18499/v1")
    p.add_argument("--minicpm-model", default="minicpm-ocr")
    p.add_argument("--minicpm-mode", choices=("page", "sdk"), default="page")
    p.add_argument("--minicpm-backend", choices=("llama", "vllm"), default="llama")
    p.add_argument("--minicpm-md", help="directory of MiniCPM Markdown to score instead of calling the server")
    p.add_argument("--sdk-bin", help="minicpmvocr executable for --minicpm-mode sdk")
    p.add_argument("--jev-url", default="http://127.0.0.1:18500")
    p.add_argument("--jev-protocol", choices=("rm", "score"), default="rm")
    p.add_argument("--jev-on", default="baseline,minicpm", help="comma list: baseline, minicpm")
    p.add_argument("--skip-minicpm", action="store_true")
    p.add_argument("--skip-jev", action="store_true")
    p.add_argument("--only", help="basename of one scan, for a smoke test")
    p.add_argument("--dpi", type=int, default=144, help="MiniCPM page render DPI. 144 matches the SDK zoom of 2.")
    p.add_argument("--baseline-dpi", type=int, default=None, help="passed to ocr scan --dpi. Omit to keep that engine's own default (150 for paddleocr-vl).")
    p.add_argument("--max-tokens", type=int, default=8192)
    p.add_argument("--timeout", type=float, default=900)
    p.add_argument("--layout-dir", help="optional {stem}.json layouts so Jev sees crops instead of the whole page")
    p.add_argument("--use-real-config", action="store_true", help="use the ocr config the server was started with, not <out>/state")
    p.add_argument("--ocr-bin", default=None)
    p.add_argument("--dry-run", action="store_true")
    return p


def execute(args):
    out = assert_output_allowed(args.out)
    input_dirs = [Path(d) for d in args.input_dir]
    ref_dirs = [Path(d) for d in (args.reference_dir or args.input_dir)]
    scans = reference.iter_scans(input_dirs)
    if args.only:
        scans = [p for p in scans if p.name == args.only or p.stem == args.only]
        if not scans:
            raise CompareError(f"no scan named {args.only} under {', '.join(args.input_dir)}")
    refs, warnings = reference.load_references(scans, ref_dirs)
    for w in warnings:
        print(f"warning: {w}", file=sys.stderr)

    jev_on = {p.strip() for p in args.jev_on.split(",") if p.strip()}
    run_minicpm = not args.skip_minicpm
    run_jev = not args.skip_jev
    run_baseline = args.baseline_md is None
    if run_minicpm and not args.minicpm_md:
        clients.require_local(args.minicpm_url)
    if run_jev:
        clients.require_local(args.jev_url)
    ocr_bin = args.ocr_bin or shutil.which("ocr") or str(_ROOT / "target" / "release" / "ocr")
    if run_baseline and not Path(ocr_bin).exists() and shutil.which(ocr_bin) is None:
        raise CompareError(f"ocr binary not found ({ocr_bin}). Pass --ocr-bin or --baseline-md.")

    plan = {
        "scans": [str(p) for p in scans],
        "references": {stem: str(ref.path) for stem, ref in refs.items()},
        "baseline": "saved markdown" if args.baseline_md else f"ocr scan --model {args.baseline_model}",
        "minicpm": None if not run_minicpm else (args.minicpm_md or args.minicpm_url),
        "jev": None if not run_jev else args.jev_url,
        "out": str(out),
        "warnings": len(warnings),
    }
    if args.dry_run:
        print(json.dumps(plan, indent=2))
        return 0

    out.mkdir(parents=True, exist_ok=True)
    env = _isolated_env(out, args.use_real_config) if run_baseline else None
    files = []
    buckets = {
        "baseline": {},
        "minicpm": {},
    }
    jev_parts = {"baseline": [], "minicpm": []}
    dis_only_b = Counter()
    dis_only_m = Counter()
    dis_shared = Counter()
    disagreed = False

    for scan in scans:
        tag = _rel_tag(scan, input_dirs)
        ref = refs.get(scan.stem)
        entry = {
            "file": tag.as_posix(),
            "reference": None if ref is None else str(ref.path),
            "reference_format": None if ref is None else ref.format,
        }
        base_md_path = _find_saved(args.baseline_md, tag) if args.baseline_md else _md_path(out / "baseline", tag)
        mini_md_path = _find_saved(args.minicpm_md, tag) if args.minicpm_md else _md_path(out / "minicpm", tag)
        try:
            if run_baseline:
                _run_baseline(scan, base_md_path, args.baseline_model, args.baseline_dpi, ocr_bin, env)
            base_text = base_md_path.read_text(encoding="utf-8") if base_md_path and base_md_path.exists() else None
        except (OSError, RuntimeError) as exc:
            base_text = None
            entry["baseline_error"] = str(exc)[:400]
        if base_text is None and "baseline_error" not in entry:
            entry["baseline_error"] = f"no baseline markdown at {base_md_path}"
        if base_text is not None:
            bodies = _page_bodies(base_text)
            speed = None
            timing = _timing_file(base_md_path) if base_md_path else None
            if timing and timing.exists():
                speed = _speed_from_timing(timing)
            else:
                speed = _speed(len(bodies), None, [])
            part = {"speed": speed, "text": _score_text(ref, base_text), "fields": _score_fields(ref, bodies)}
            entry["baseline"] = {k: v for k, v in part.items() if k != "text"} | {"text": part["text"]}
            _accumulate(buckets["baseline"], {"speed": speed, **(part["text"] or {}), "fields": part["fields"]})
        else:
            bodies = []

        mini_bodies = []
        if run_minicpm:
            try:
                if args.minicpm_md:
                    mini_text = mini_md_path.read_text(encoding="utf-8")
                    speed = _speed(len(_page_bodies(mini_text)), None, [])
                elif args.minicpm_mode == "sdk":
                    mini_md_path.parent.mkdir(parents=True, exist_ok=True)
                    mini_text, wall = clients.recognize_sdk(
                        scan, mini_md_path, args.minicpm_url, args.minicpm_model, args.sdk_bin, [],
                    )
                    speed = _speed(max(mini_text.count("<!-- page"), 1), wall, [])
                    mini_md_path.write_text(mini_text, encoding="utf-8")
                else:
                    mini_text, records, wall = clients.recognize_pages(
                        scan, args.minicpm_url, args.minicpm_model, args.dpi,
                        args.max_tokens, args.minicpm_backend, args.timeout,
                    )
                    mini_md_path.parent.mkdir(parents=True, exist_ok=True)
                    mini_md_path.write_text(mini_text, encoding="utf-8")
                    speed = _speed(len(records), wall, [r.get("wall_s") for r in records])
                    entry["minicpm_truncated"] = sum(1 for r in records if r.get("truncated"))
                mini_bodies = _page_bodies(mini_text)
                part = {"speed": speed, "text": _score_text(ref, mini_text), "fields": _score_fields(ref, mini_bodies)}
                entry["minicpm"] = part
                _accumulate(buckets["minicpm"], {"speed": speed, **(part["text"] or {}), "fields": part["fields"]})
            except (OSError, RuntimeError, ValueError) as exc:
                entry["minicpm_error"] = str(exc)[:400]

        if ref is None and bodies and mini_bodies:
            dis = metrics.disagreement(bodies, mini_bodies)
            disagreed = True
            for kind, cell in dis["by_kind"].items():
                dis_only_b[kind] += cell["only_baseline"]
                dis_only_m[kind] += cell["only_minicpm"]
                dis_shared[kind] += cell["shared"]
            entry["disagreement"] = {
                "only_baseline": dis["only_baseline"],
                "only_minicpm": dis["only_minicpm"],
                "shared": dis["shared"],
                "by_kind": dis["by_kind"],
            }

        if run_jev:
            entry["jev"] = {}
            if "baseline" in jev_on and bodies:
                raw = _jev_on_pages(scan, bodies, ref, args.jev_url, args.jev_protocol, args.dpi, args.timeout, args.layout_dir)
                entry["jev"]["baseline"] = _strip_text(raw)
                jev_parts["baseline"].append(entry["jev"]["baseline"])
            if "minicpm" in jev_on and mini_bodies:
                raw = _jev_on_pages(scan, mini_bodies, ref, args.jev_url, args.jev_protocol, args.dpi, args.timeout, args.layout_dir)
                entry["jev"]["minicpm"] = _strip_text(raw)
                jev_parts["minicpm"].append(entry["jev"]["minicpm"])
        # Drop hypothesis text from the JSON. Scores stay.
        files.append(entry)
        print(f"{tag.as_posix()}: reference={'yes' if ref else 'no'}", file=sys.stderr, flush=True)

    def engine_summary(key):
        bucket = buckets[key]
        acc = {k: bucket.get(k, 0) for k in ("dist_sum", "len_sum", "wdist", "wlen")}
        return {
            "accuracy": acc,
            "fields": _merge_fields(bucket.get("field_rows") or []),
            "speed": _merge_speed(bucket.get("speeds") or []),
        }

    disagreement = None
    if disagreed:
        disagreement = {
            "only_baseline": sum(dis_only_b.values()),
            "only_minicpm": sum(dis_only_m.values()),
            "shared": sum(dis_shared.values()),
            "by_kind": {
                kind: {
                    "only_baseline": dis_only_b[kind],
                    "only_minicpm": dis_only_m[kind],
                    "shared": dis_shared[kind],
                }
                for kind in sorted(set(dis_only_b) | set(dis_only_m) | set(dis_shared))
            },
        }

    report = {
        "schema": "minicpm-compare-1",
        "privacy": "Canonical field values may appear in this file. It is gitignored. Do not commit it or paste page text.",
        "created": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "baseline_model": args.baseline_model,
        "minicpm_mode": args.minicpm_mode,
        "minicpm_backend": args.minicpm_backend,
        "files_total": len(files),
        "files_with_reference": sum(1 for f in files if f.get("reference")),
        "files_without_reference": sum(1 for f in files if not f.get("reference")),
        "warnings": warnings,
        "skipped": {"minicpm": not run_minicpm, "baseline": False, "jev": not run_jev},
        "engines": {
            "baseline": engine_summary("baseline"),
            "minicpm": engine_summary("minicpm") if run_minicpm else {},
        },
        "disagreement": disagreement,
        "jev": {
            "baseline": _merge_jev(jev_parts["baseline"]) if run_jev else None,
            "minicpm": _merge_jev(jev_parts["minicpm"]) if run_jev else None,
        },
        "files": files,
        "plan": plan,
    }
    (out / "results.json").write_text(json.dumps(report, indent=1) + "\n", encoding="utf-8")
    (out / "summary.md").write_text(render_summary(report), encoding="utf-8")
    print(f"wrote {out / 'results.json'}", file=sys.stderr)
    print(f"wrote {out / 'summary.md'}", file=sys.stderr)
    failed = sum(1 for f in files if f.get("baseline_error") or f.get("minicpm_error"))
    return 1 if failed else 0


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        code = execute(args)
    except clients.RemoteURL as exc:
        print(str(exc), file=sys.stderr)
        return 2
    return code


if __name__ == "__main__":
    sys.exit(main())
