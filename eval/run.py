#!/usr/bin/env python3
"""Run `ocr scan` over an eval set (made by generate.py), then score it.

usage: run.py --engine apple|paddleocr-vl|olmocr|dots-ocr --set DIR --out RESULTS_DIR
              [--dpi N] [--extra-args "..."] [--families a,b] [--use-real-config] [--ocr-bin PATH]

Config: `apple` runs with its own empty OCR_CONFIG_DIR/OCR_STATE_DIR under RESULTS_DIR/_env/<engine>
so it never touches the user's setup. Model engines need a running server (`ocr server start`) and
the real config/state, so they default to the real config; the server's pid comes from
<state>/server.json (src/server.rs). Pass --use-real-config to force the real config for apple too.

Memory: the `ocr` process peak comes from `/usr/bin/time -l` ("peak memory footprint", macOS). For
model engines the server's "Physical footprint (peak)" is read ONCE after all fixtures with
`vmmap --summary <pid>` (never polled during the run, vmmap stalls the target).

Results go to RESULTS_DIR/<engine>.json, MERGED per family with any earlier run of the same engine
(so a non-production run and a production run can be combined); outputs are in RESULTS_DIR/<engine>/.
"""
import argparse, json, os, re, shlex, subprocess, sys, time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import score as scorer  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
FAMILIES = ["pleading", "contract", "transcript4up", "fax", "table", "production"]
MODEL_ENGINES = {"paddleocr-vl", "olmocr", "dots-ocr"}


def to_mb(s):
    n, u = float(s[:-1]), s.strip()[-1]
    return n * {"B": 1 / 1048576, "K": 1 / 1024, "M": 1, "G": 1024}[u]


def server_peak_mb(state_dir):
    """Peak physical footprint of the running server in MB, or None."""
    try:
        pid = json.load(open(Path(state_dir) / "server.json"))["pid"]
    except (OSError, KeyError, ValueError):
        return None
    r = subprocess.run(["vmmap", "--summary", str(pid)], capture_output=True, text=True)
    m = re.search(r"Physical footprint \(peak\):\s+([\d.]+[BKMG])", r.stdout)
    return round(to_mb(m.group(1)), 1) if m else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--engine", required=True, choices=["apple", "paddleocr-vl", "olmocr", "dots-ocr"])
    ap.add_argument("--set", required=True, dest="set_dir")
    ap.add_argument("--out", required=True)
    ap.add_argument("--dpi", type=int)
    ap.add_argument("--extra-args", default="")
    ap.add_argument("--families", help="comma list (default: every family found in --set)")
    ap.add_argument("--use-real-config", action="store_true")
    ap.add_argument("--ocr-bin", default=str(ROOT / "target/release/ocr"))
    a = ap.parse_args()

    set_dir, out = Path(a.set_dir).resolve(), Path(a.out).resolve()
    fams = a.families.split(",") if a.families else [f for f in FAMILIES if (set_dir / f"{f}.pdf").exists()]
    (out / a.engine).mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    real = a.use_real_config or a.engine in MODEL_ENGINES
    if not real:
        base = out / "_env" / a.engine
        env["OCR_CONFIG_DIR"], env["OCR_STATE_DIR"] = str(base / "config"), str(base / "state")
    state_dir = env.get("OCR_STATE_DIR") or str(Path.home() / "Library/Application Support/ocr")

    res_path = out / f"{a.engine}.json"
    result = json.load(open(res_path)) if res_path.exists() else {"engine": a.engine, "families": {}}
    result.update(engine=a.engine, set=str(set_dir), dpi=a.dpi, extra_args=a.extra_args)

    for fam in fams:
        pdf, gtp, md = set_dir / f"{fam}.pdf", set_dir / f"{fam}.gt.json", out / a.engine / f"{fam}.md"
        gt = json.load(open(gtp))
        cmd = ["/usr/bin/time", "-l", a.ocr_bin, "scan", "--input-file", str(pdf), "--output-file", str(md),
               "--model", a.engine, "--overwrite", "--quiet"]
        if a.dpi:
            cmd += ["--dpi", str(a.dpi)]
        cmd += shlex.split(a.extra_args)
        t0 = time.perf_counter()
        p = subprocess.run(cmd, env=env, capture_output=True, text=True)
        wall = time.perf_counter() - t0
        m = re.search(r"(\d+)\s+peak memory footprint", p.stderr)
        entry = {"wall_s": round(wall, 2), "pages": len(gt["pages"]), "exit_code": p.returncode,
                 "pages_per_s": round(len(gt["pages"]) / wall, 3),
                 "ocr_peak_mb": round(int(m.group(1)) / 1048576, 1) if m else None}
        if p.returncode != 0 or not md.exists():
            entry["error"] = p.stderr.strip()[-500:]
        else:
            s = scorer.score(gt, md.read_text(), strip_line_numbers=(fam == "pleading"))
            entry.update({k: s[k] for k in ("cer", "token_recall", "bates_accuracy", "transcript_line_accuracy",
                                            "pages_in_output")})
            entry["cer_pages"] = s["cer_pages"]
        result["families"][fam] = entry
        print(f"{fam}: {entry.get('cer')} CER, {wall:.1f}s", file=sys.stderr)

    if a.engine in MODEL_ENGINES:
        result["server_peak_mb"] = server_peak_mb(state_dir)
    json.dump(result, open(res_path, "w"), indent=1)

    f = lambda v, pct=True: "-" if v is None else (f"{v * 100:.1f}%" if pct else f"{v}")
    print(f"\n| family | pages | CER | token recall | Bates | transcript lines | pages/s | wall s | ocr peak MB |")
    print("|---|---|---|---|---|---|---|---|---|")
    for fam in fams:
        e = result["families"][fam]
        print(f"| {fam} | {e['pages']} | {f(e.get('cer'))} | {f(e.get('token_recall'))} | {f(e.get('bates_accuracy'))} | "
              f"{f(e.get('transcript_line_accuracy'))} | {e['pages_per_s']} | {e['wall_s']} | {e['ocr_peak_mb']} |"
              + (f" ERROR {e['error'][:80]}" if "error" in e else ""))
    if a.engine in MODEL_ENGINES:
        print(f"\nServer peak physical footprint: {result.get('server_peak_mb')} MB")


if __name__ == "__main__":
    main()
