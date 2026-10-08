"""Paddle engine spike through the production render path (examples/render_png).

usage: paddle_spike.py <port>   (run from repo root; needs the PaddleOCR-VL-1.6 weights cached)
"""
import base64, glob, json, os, re, signal, subprocess, sys, time, urllib.request
port = sys.argv[1]
REPO = "PaddlePaddle/PaddleOCR-VL-1.6"
OUT = "spike/out/paddle_spike"
os.makedirs(OUT, exist_ok=True)
log = open(f"{OUT}/server.log", "w")
srv = subprocess.Popen(["spike/.venv/bin/mlx_vlm.server", "--host", "127.0.0.1", "--port", port, "--model", REPO,
                        "--max-num-seqs", "1"], stdout=log, stderr=subprocess.STDOUT, start_new_session=True)

def render(pdf, page=1, dpi=200, crop=None):
    out = f"{OUT}/_r.png"
    cmd = ["target/release/examples/render_png", pdf, str(page), str(dpi), out] + ([",".join(map(str, crop))] if crop else [])
    subprocess.run(cmd, check=True)
    return open(out, "rb").read()

def chat(png, prompt="OCR:", max_tokens=4096):
    body = {"model": REPO, "max_tokens": max_tokens, "temperature": 0, "messages": [{"role": "user", "content": [
        {"type": "image_url", "image_url": {"url": "data:image/png;base64," + base64.b64encode(png).decode()}},
        {"type": "text", "text": prompt}]}]}
    t = time.time()
    r = json.load(urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions",
        json.dumps(body).encode(), {"Content-Type": "application/json"}), timeout=900))
    return r["choices"][0]["message"]["content"], r["choices"][0].get("finish_reason"), time.time() - t

TOKENS = ["3:26-cv-04417", "$48,250.00", "$51,900.00", "$22,375.50", "$122,525.50", "SMITH000123"]
try:
    for _ in range(300):
        try: urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2); break
        except Exception: time.sleep(1)
    chat(render("spike/fixtures/legal.pdf", dpi=72), max_tokens=8)  # warm-up

    print("## 1. Legal fixtures, full page")
    for f in ("legal", "legal_scan"):
        for dpi in (150, 200, 300):
            text, fin, dt = chat(render(f"spike/fixtures/{f}.pdf", dpi=dpi))
            miss = [t for t in TOKENS if t not in text]
            print(f"{f} @{dpi}: {dt:.1f}s finish={fin} missing={miss}")
            if dpi == 200: open(f"{OUT}/{f}.txt", "w").write(text)

    print("\n## 2. Columns, full page (no crops)")
    text, fin, dt = chat(render("spike/fixtures/two_column.pdf"))
    open(f"{OUT}/two_column.txt", "w").write(text)
    seq = [f"Left column sentence {i} concerns" for i in range(1, 16)] + [f"Right column sentence {i} concerns" for i in range(1, 16)]
    found = [text.find(s) for s in seq if s in text]
    print(f"two_column: found {len(found)}/30, in-order {sum(b > a for a, b in zip(found, found[1:]))}/{max(len(found)-1,0)} ({dt:.1f}s)")
    for seed in (0, 1, 2):
        d = f"spike/out/paddle_spike/seed{seed}"
        for pdf in sorted(glob.glob(f"{d}/*.pdf")):
            text, fin, dt = chat(render(pdf))
            open(pdf[:-4] + ".md", "w").write(text)
    print("(held-out scored below)")

    print("\n## 3. Table band split vs full page on legal_scan (Vision table bbox [0.167,0.396,0.624,0.108])")
    full = open(f"{OUT}/legal_scan.txt").read()
    above, _, _ = chat(render("spike/fixtures/legal_scan.pdf", crop=[0, 0, 1, 0.386]))
    table, fin, _ = chat(render("spike/fixtures/legal_scan.pdf", crop=[0.15, 0.386, 0.66, 0.128]), "Table Recognition:", 2048)
    below, _, _ = chat(render("spike/fixtures/legal_scan.pdf", crop=[0, 0.514, 1, 0.486]))
    banded = above + "\n" + table + "\n" + below
    open(f"{OUT}/legal_scan_banded.txt", "w").write(banded)
    print("full missing:", [t for t in TOKENS if t not in full])
    print("banded missing:", [t for t in TOKENS if t not in banded])
    print("table crop output:", table[:300])
finally:
    os.killpg(srv.pid, signal.SIGTERM)
    srv.wait(20)

done = [l for l in open(f"{OUT}/server.log") if "Request completed" in l]
rates = [(float(re.search(r"prefill=([\d.]+)", l).group(1)), float(re.search(r"decode=([\d.]+)", l).group(1)),
          int(re.search(r"prompt_tokens=(\d+)", l).group(1)), int(re.search(r"generated_tokens=(\d+)", l).group(1))) for l in done[1:]]
print("\n## 4. Timing (prefill tok/s, decode tok/s, prompt tok, gen tok) per request:")
for r in rates: print("  ", r)
