"""Measure physical footprint (vmmap) of mlx_vlm.server per engine, after one real page."""
import base64, json, os, re, signal, subprocess, sys, threading, time, urllib.request
from pathlib import Path
HERE = Path(__file__).parent
repo, image, port = sys.argv[1], sys.argv[2], sys.argv[3]
p = subprocess.Popen([str(HERE/".venv/bin/mlx_vlm.server"), "--host", "127.0.0.1", "--port", port, "--model", repo],
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
peak = [0.0]; stop = threading.Event()
def fp():
    out = subprocess.run(["vmmap", "--summary", str(p.pid)], capture_output=True, text=True).stdout
    m = re.search(r"Physical footprint \(peak\):\s+([\d.]+)([KMG])", out)
    if m:
        v = float(m.group(1)) * {"K": 1/2**20, "M": 1/1024, "G": 1}[m.group(2)]
        peak[0] = max(peak[0], v)
try:
    while True:
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2); break
        except Exception: time.sleep(0.5)
    img = base64.b64encode((HERE/"fixtures"/image).read_bytes()).decode()
    body = {"model": repo, "max_tokens": 1024, "temperature": 0, "messages": [{"role": "user", "content": [
        {"type": "image_url", "image_url": {"url": "data:image/png;base64," + img}}, {"type": "text", "text": "OCR:"}]}]}
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions", json.dumps(body).encode(),
                           {"Content-Type": "application/json"}), timeout=600).read()
    fp()
    print(repo, "physical footprint peak:", round(peak[0], 2), "GB")
finally:
    os.killpg(p.pid, signal.SIGTERM); p.wait(20)
