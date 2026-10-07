"""Phase 0a spike: start mlx_vlm.server for one engine and measure what PLAN.md needs.

usage: bench.py <engine> <repo> <port> [--trust-remote-code]
Writes out/<engine>/*.txt and out/<engine>/report.json.
"""
import base64, concurrent.futures as cf, io, json, os, signal, subprocess, sys, threading, time
import urllib.request, urllib.error
from pathlib import Path

import psutil
from PIL import Image

HERE = Path(__file__).parent
FIX = HERE / "fixtures"
UP = HERE / "upstream"

engine, repo, port = sys.argv[1], sys.argv[2], int(sys.argv[3])
trust = "--trust-remote-code" in sys.argv
out = HERE / "out" / engine
out.mkdir(parents=True, exist_ok=True)
base = f"http://127.0.0.1:{port}"

OLMOCR_PROMPT = (
    "Attached is one page of a document that you must process. "
    "Just return the plain text representation of this document as if you were reading it naturally. Convert equations to LateX and tables to HTML.\n"
    "If there are any figures or charts, label them with the following markdown syntax ![Alt text describing the contents of the figure](page_startx_starty_width_height.png)\n"
    "Return your output as markdown, with a front matter section on top specifying values for the primary_language, is_rotation_valid, rotation_correction, is_table, and is_diagram parameters."
)
ns = {}
exec((UP / "rednote-hilab_dots.mocr.py").read_text(), ns)
DOTS = ns["dict_promptmode_to_prompt"]

CFG = {
    "olmocr": dict(prompt=OLMOCR_PROMPT, image="legal_1288.png", max_tokens=4096, text_first=True),
    "dots": dict(prompt=DOTS["prompt_layout_all_en"], image="legal_200dpi.png", max_tokens=8192, text_first=False),
    "dots-ocr": dict(prompt=DOTS["prompt_ocr"], image="legal_200dpi.png", max_tokens=4096, text_first=False),
    "paddle": dict(prompt="OCR:", image="legal_200dpi.png", max_tokens=4096, text_first=False),
    "paddle-table": dict(prompt="Table Recognition:", image="table_crop.png", max_tokens=2048, text_first=False),
}[engine.split("@")[0]]


def variant(k: int) -> bytes:
    """Same page shifted by k px so the server's vision-feature cache can't short-circuit."""
    img = Image.open(FIX / CFG["image"]).convert("RGB")
    if k:
        canvas = Image.new("RGB", img.size, "white")
        canvas.paste(img, (k, k))
        img = canvas
    buf = io.BytesIO()
    img.save(buf, "PNG")
    return buf.getvalue()


def chat(png: bytes, prompt: str, max_tokens: int, timeout: float = 600):
    img = {"type": "image_url", "image_url": {"url": "data:image/png;base64," + base64.b64encode(png).decode()}}
    txt = {"type": "text", "text": prompt}
    body = {
        "model": repo,
        "messages": [{"role": "user", "content": [txt, img] if CFG["text_first"] else [img, txt]}],
        "max_tokens": max_tokens,
        "temperature": 0,
    }
    req = urllib.request.Request(base + "/v1/chat/completions", json.dumps(body).encode(), {"Content-Type": "application/json"})
    t = time.time()
    with urllib.request.urlopen(req, timeout=timeout) as r:
        resp = json.load(r)
    ch = resp["choices"][0]
    return ch["message"]["content"], ch.get("finish_reason"), resp.get("usage", {}), time.time() - t


def get(path):
    with urllib.request.urlopen(base + path, timeout=5) as r:
        return r.status


report = {"engine": engine, "repo": repo}
log = open(out / "server.log", "w")
cmd = [str(HERE / ".venv/bin/mlx_vlm.server"), "--host", "127.0.0.1", "--port", str(port), "--model", repo]
if trust:
    cmd.append("--trust-remote-code")
t0 = time.time()
proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)

peak = {"rss": 0}
stop = threading.Event()
def sample():
    while not stop.is_set():
        try:
            p = psutil.Process(proc.pid)
            rss = p.memory_info().rss + sum(c.memory_info().rss for c in p.children(recursive=True))
            peak["rss"] = max(peak["rss"], rss)
        except psutil.Error:
            pass
        time.sleep(0.2)
threading.Thread(target=sample, daemon=True).start()

try:
    while True:
        if proc.poll() is not None:
            raise SystemExit(f"server exited rc={proc.returncode}; see {out/'server.log'}")
        try:
            if get("/health") == 200:
                break
        except Exception:
            time.sleep(0.5)
    report["health_after_s"] = round(time.time() - t0, 1)

    # Readiness probe: tiny real generation.
    tiny = Image.new("RGB", (64, 64), "white"); b = io.BytesIO(); tiny.save(b, "PNG")
    _, _, _, dt = chat(b.getvalue(), CFG["prompt"], 8)
    report["probe_ready_after_s"] = round(time.time() - t0, 1)

    # 1. Sequential baseline (variant 0), twice, to see determinism.
    text0, fin0, usage0, dt0 = chat(variant(0), CFG["prompt"], CFG["max_tokens"])
    (out / "seq.txt").write_text(text0)
    text0b, *_ , dt0b = chat(variant(0), CFG["prompt"], CFG["max_tokens"])
    report["sequential"] = {"seconds": round(dt0, 2), "seconds_repeat": round(dt0b, 2), "finish": fin0, "usage": usage0,
                            "deterministic": text0 == text0b}

    # 2. Concurrency on distinct (shifted) images.
    seq1 = chat(variant(1), CFG["prompt"], CFG["max_tokens"])[0]
    for n in (2, 4):
        imgs = [variant(i + 1) for i in range(n)]
        t = time.time()
        with cf.ThreadPoolExecutor(n) as ex:
            res = list(ex.map(lambda im: chat(im, CFG["prompt"], CFG["max_tokens"]), imgs))
        wall = time.time() - t
        report[f"concurrent_{n}"] = {
            "wall_s": round(wall, 2),
            "speedup_vs_serial": round(n * dt0 / wall, 2),
            "variant1_matches_sequential": res[0][0] == seq1,
            "finish": [r[1] for r in res],
        }
        (out / f"conc{n}_v1.txt").write_text(res[0][0])

    # 3. Cancellation: abandon a long request, then time a short one.
    def abandoned():
        try:
            chat(variant(9), CFG["prompt"], 8192, timeout=2)
        except Exception:
            pass
    th = threading.Thread(target=abandoned); th.start(); th.join()
    _, _, _, short_dt = chat(b.getvalue(), CFG["prompt"], 8)
    report["cancellation"] = {"short_request_after_abandon_s": round(short_dt, 2), "baseline_short_s": round(dt, 2)}
finally:
    stop.set()
    report["peak_rss_gb"] = round(peak["rss"] / 2**30, 2)
    os.killpg(proc.pid, signal.SIGTERM)
    try:
        proc.wait(15)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
    (out / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
