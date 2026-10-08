"""Paddle on Phase 2b column crops vs full page. usage: paddle_crops.py <port> <pdf> <x,y,w,h>..."""
import base64, json, os, signal, subprocess, sys, time, urllib.request
port, pdf, crops = sys.argv[1], sys.argv[2], sys.argv[3:]
REPO = "PaddlePaddle/PaddleOCR-VL-1.6"
srv = subprocess.Popen(["spike/.venv/bin/mlx_vlm.server", "--host", "127.0.0.1", "--port", port, "--model", REPO, "--max-num-seqs", "1"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
def chat(crop):
    subprocess.run(["target/release/examples/render_png", pdf, "1", "200", "/tmp/_c.png"] + ([crop] if crop else []), check=True)
    body = {"model": REPO, "max_tokens": 4096, "temperature": 0, "messages": [{"role": "user", "content": [
        {"type": "image_url", "image_url": {"url": "data:image/png;base64," + base64.b64encode(open("/tmp/_c.png", "rb").read()).decode()}},
        {"type": "text", "text": "OCR:"}]}]}
    r = json.load(urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions", json.dumps(body).encode(), {"Content-Type": "application/json"}), timeout=900))
    return r["choices"][0]["message"]["content"], r["choices"][0]["finish_reason"]
try:
    for _ in range(300):
        try: urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2); break
        except Exception: time.sleep(1)
    exp = json.load(open(pdf.replace("_scan.pdf", ".pdf")[:-4] + ".expected.json"))
    out = ""
    for c in crops:
        t, fin = chat(c); out += t + "\n"; print("crop", c, "finish", fin)
    found = [out.find(s) for s in exp if s in out]
    print(f"crops: found {len(found)}/{len(exp)} in-order {sum(b > a for a, b in zip(found, found[1:]))}/{max(len(found)-1,0)}")
    print(out[:600])
finally:
    os.killpg(srv.pid, signal.SIGTERM)
