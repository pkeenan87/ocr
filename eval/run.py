#!/usr/bin/env python3
"""Run `ocr scan` over one or more eval sets (made by generate.py), then score them.

usage: run.py --engine apple|paddleocr-vl|olmocr|dots-ocr --set DIR [DIR ...] --out RESULTS_DIR
              [--tag NAME] [--dpi N] [--extra-args "..."] [--families a,b] [--keep-furniture]
              [--use-real-config] [--ocr-bin PATH] [--git-rev HASH]

Config: every engine runs with the ISOLATED state under eval/out/state/{config,state}
(OCR_CONFIG_DIR / OCR_STATE_DIR), never the user's real ~/.config/ocr. `--use-real-config` is the
opt-in escape hatch. Model engines need a server running in that isolated state
(`OCR_CONFIG_DIR=... OCR_STATE_DIR=... ocr server start --model X`); its pid comes from
<state>/server.json (src/server.rs).

Scoring flags are the same for every engine: --strip-line-numbers on the `pleading` family only,
furniture (headers, footers, Bates strings from the ground truth) ignored unless --keep-furniture.

Memory: the `ocr` process peak comes from `/usr/bin/time -l` ("peak memory footprint", macOS). For
model engines the server's "Physical footprint (peak)" is read ONCE after all runs with
`vmmap --summary <pid>` (never polled during the run, vmmap stalls the target). It is a peak since
the server started, so restart the server per measurement if you need a per-run figure.

Results: RESULTS_DIR/<tag>.json (tag defaults to the engine), one entry per family and set (seed),
MERGED with earlier runs of the same tag; outputs in RESULTS_DIR/<tag>/<set name>/<family>.md.
The printed table aggregates over the sets given in THIS invocation (micro-averaged: total edit
distance / total characters, total tokens found / total tokens, pages / total wall seconds).
"""
import argparse, json, os, re, shlex, subprocess, sys, time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import score as scorer  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
ISO_CONFIG, ISO_STATE = ROOT / "eval/out/state/config", ROOT / "eval/out/state/state"
FAMILIES = ["pleading", "contract", "transcript4up", "fax", "table", "production"]
MODEL_ENGINES = {"paddleocr-vl", "olmocr", "dots-ocr"}


def to_mb(s):
    n, u = float(s[:-1]), s.strip()[-1]
    return n * {"B": 1 / 1048576, "K": 1 / 1024, "M": 1, "G": 1024}[u]


def server_pid(state_dir):
    try:
        return json.load(open(Path(state_dir) / "server.json"))["pid"]
    except (OSError, KeyError, ValueError):
        return None


def server_peak_mb(state_dir):
    """Peak memory of the running server in MB, or None: the physical footprint peak from
    `vmmap` on macOS, the resident peak (VmHWM) on Linux. GPU memory is `server_vram_mb`."""
    pid = server_pid(state_dir)
    if pid is None:
        return None
    if sys.platform == "darwin":
        r = subprocess.run(["vmmap", "--summary", str(pid)], capture_output=True, text=True)
        m = re.search(r"Physical footprint \(peak\):\s+([\d.]+[BKMG])", r.stdout)
        return round(to_mb(m.group(1)), 1) if m else None
    try:
        m = re.search(r"VmHWM:\s+(\d+) kB", open(f"/proc/{pid}/status").read())
    except OSError:
        return None
    return round(int(m.group(1)) / 1024, 1) if m else None


def server_vram_mb(state_dir):
    """GPU memory the server holds now (NVIDIA, via nvidia-smi), in MB, or None."""
    pid = server_pid(state_dir)
    try:
        r = subprocess.run(["nvidia-smi", "--query-compute-apps=pid,used_memory", "--format=csv,noheader,nounits"],
                           capture_output=True, text=True)
    except OSError:
        return None
    for line in r.stdout.splitlines():
        p, _, mb = line.partition(",")
        if p.strip() == str(pid):
            return float(mb)
    return None


def child_peak_mb():
    """Largest resident peak of any child so far (off macOS, where BSD `time -l` is missing)."""
    try:
        import resource
    except ImportError:  # Windows
        return None
    return round(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024, 1)  # Linux: KiB


def aggregate(entries):
    ok = [e for e in entries if "error" not in e]
    if not ok:
        return {}
    g = lambda k: sum(e[k] for e in ok)
    ratio = lambda a, b: round(g(a) / g(b), 4) if g(b) else None
    return {"sets": len(entries), "failed": len(entries) - len(ok), "pages": g("pages"),
            "cer": ratio("dist_sum", "len_sum"), "token_recall": ratio("tokens_found", "tokens_total"),
            "bates_accuracy": ratio("bates_ok", "bates_total"),
            "transcript_line_accuracy": ratio("tl_ok", "tl_total"),
            "pages_per_s": round(g("pages") / g("wall_s"), 3), "wall_s": round(g("wall_s"), 1),
            "ocr_peak_mb": max(e["ocr_peak_mb"] or 0 for e in ok),
            "crashes": sum(e.get("crashes", 0) for e in entries)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--engine", required=True, choices=["apple", "paddleocr-vl", "olmocr", "dots-ocr"])
    ap.add_argument("--set", required=True, nargs="+", dest="set_dirs")
    ap.add_argument("--out", required=True)
    ap.add_argument("--tag")
    ap.add_argument("--dpi", type=int)
    ap.add_argument("--extra-args", default="")
    ap.add_argument("--families", help="comma list (default: every family found in each set except production)")
    ap.add_argument("--first-pages", type=int, help="scan only pages 1-N (--pages 1-N) and score against the first N ground-truth pages")
    ap.add_argument("--keep-furniture", action="store_true", help="score headers/footers/Bates too")
    ap.add_argument("--use-real-config", action="store_true")
    ap.add_argument("--ocr-bin", default=str(ROOT / "target/release" / ("ocr.exe" if os.name == "nt" else "ocr")))
    ap.add_argument("--git-rev", default=subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
                                                        capture_output=True, text=True).stdout.strip())
    a = ap.parse_args()
    tag = a.tag or a.engine

    out = Path(a.out).resolve()
    (out / tag).mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    if not a.use_real_config:
        env["OCR_CONFIG_DIR"], env["OCR_STATE_DIR"] = str(ISO_CONFIG), str(ISO_STATE)
    state_dir = env.get("OCR_STATE_DIR") or str(Path.home() / "Library/Application Support/ocr")

    res_path = out / f"{tag}.json"
    result = json.load(open(res_path)) if res_path.exists() else {"tag": tag, "runs": {}}
    result.update(engine=a.engine, git_rev=a.git_rev, dpi=a.dpi, extra_args=a.extra_args,
                  keep_furniture=a.keep_furniture)
    this_run = {}

    for sd in a.set_dirs:
        set_dir = Path(sd).resolve()
        fams = a.families.split(",") if a.families else [
            f for f in FAMILIES if f != "production" and (set_dir / f"{f}.pdf").exists()]
        (out / tag / set_dir.name).mkdir(parents=True, exist_ok=True)
        for fam in fams:
            pdf, gtp, md = set_dir / f"{fam}.pdf", set_dir / f"{fam}.gt.json", out / tag / set_dir.name / f"{fam}.md"
            gt = json.load(open(gtp))
            if a.first_pages:
                gt["pages"] = gt["pages"][:a.first_pages]
            # BSD `time -l` gives the peak footprint on macOS; elsewhere `child_peak_mb`.
            timer = ["/usr/bin/time", "-l"] if sys.platform == "darwin" else []
            cmd = timer + [a.ocr_bin, "scan", "--input-file", str(pdf), "--output-file", str(md),
                   "--model", a.engine, "--overwrite", "--quiet"]
            if a.dpi:
                cmd += ["--dpi", str(a.dpi)]
            if a.first_pages:
                cmd += ["--pages", f"1-{a.first_pages}"]
            cmd += shlex.split(a.extra_args)
            busy = subprocess.run("pgrep -fl 'mlx_vlm|ocr scan|cargo'", shell=True, capture_output=True, text=True).stdout
            busy = [l for l in busy.splitlines() if "run.py" not in l and "pgrep" not in l and "mlx_vlm.server" not in l]
            if busy and a.engine == "apple":
                print("WARNING busy machine:", busy, file=sys.stderr)
            # Apple Vision occasionally segfaults inside TextRecognition (objc_retain, about 1%
            # of runs, Phase 6 finding). A signal death is retried (max 3 attempts) and counted.
            crashes = 0
            while True:
                t0 = time.perf_counter()
                p = subprocess.run(cmd, env=env, capture_output=True, text=True)
                wall = time.perf_counter() - t0
                if p.returncode >= 0 or crashes >= 2:
                    break
                crashes += 1
            m = re.search(r"(\d+)\s+peak memory footprint", p.stderr)
            peak = round(int(m.group(1)) / 1048576, 1) if m else (None if timer else child_peak_mb())
            entry = {"wall_s": round(wall, 2), "pages": len(gt["pages"]), "exit_code": p.returncode,
                     "pages_per_s": round(len(gt["pages"]) / wall, 3),
                     "ocr_peak_mb": peak, "crashes": crashes}
            if p.returncode not in (0, 1) or not md.exists():
                entry["error"] = p.stderr.strip()[-500:]
            else:
                s = scorer.score(gt, md.read_text(), strip_line_numbers=(fam == "pleading"),
                                 keep_furniture=a.keep_furniture)
                entry.update({k: s[k] for k in ("cer", "dist_sum", "len_sum", "tokens_found", "tokens_total",
                                                "bates_ok", "bates_total", "tl_ok", "tl_total", "pages_in_output")})
                entry["cer_pages"] = s["cer_pages"]
                if p.returncode == 1:
                    entry["note"] = "exit 1 (some pages flagged or failed)"
            result["runs"].setdefault(fam, {})[set_dir.name] = entry
            this_run.setdefault(fam, []).append(entry)
            print(f"{set_dir.name}/{fam}: {entry.get('cer')} CER, {wall:.1f}s", file=sys.stderr, flush=True)
            json.dump(result, open(res_path, "w"), indent=1)  # checkpoint

    if a.engine in MODEL_ENGINES:
        result["server_peak_mb"] = server_peak_mb(state_dir)
        result["server_vram_mb"] = server_vram_mb(state_dir)
    result["last_summary"] = {fam: aggregate(es) for fam, es in this_run.items()}
    json.dump(result, open(res_path, "w"), indent=1)

    f = lambda v: "-" if v is None else f"{v * 100:.1f}%"
    print(f"\ngit {a.git_rev}  tag {tag}  dpi {a.dpi}  extra '{a.extra_args}'  keep_furniture {a.keep_furniture}")
    print("| family | sets | pages | CER | token recall | Bates | transcript lines | pages/s | wall s | ocr peak MB | crashes (retried) |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for fam, g in result["last_summary"].items():
        if not g:
            print(f"| {fam} | ALL FAILED |"); continue
        print(f"| {fam} | {g['sets']}{' ('+str(g['failed'])+' failed)' if g['failed'] else ''} | {g['pages']} | {f(g['cer'])} | "
              f"{f(g['token_recall'])} | {f(g['bates_accuracy'])} | {f(g['transcript_line_accuracy'])} | "
              f"{g['pages_per_s']} | {g['wall_s']} | {g['ocr_peak_mb']} | {g['crashes']} |")
    if a.engine in MODEL_ENGINES:
        print(f"\nServer peak memory: {result.get('server_peak_mb')} MB; GPU memory: {result.get('server_vram_mb')} MB")


if __name__ == "__main__":
    main()
