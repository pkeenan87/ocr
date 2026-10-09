"""Probe PaddleOCR-VL on one eval 4-up transcript sheet: full page at several DPIs vs quadrant crops.

usage: transcript_paddle_probe.py <port> <gt.json> <pdf> <sheet_index>
Scores exact "N text" line matches against ground truth (numbers must match).
"""
import base64, json, re, subprocess, sys, urllib.request
port, gt_path, pdf, sheet = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
REPO = "PaddlePaddle/PaddleOCR-VL-1.6"
gt = json.load(open(gt_path))["pages"][sheet]["text"]
norm = lambda s: re.sub(r"\s+", " ", s).strip()
want = {norm(l) for l in gt.splitlines() if re.match(r"^\s*\d{1,2}\s+\S", l)}

def ocr(dpi, crop=None):
    subprocess.run(["target/release/examples/render_png", pdf, str(sheet + 1), str(dpi), "/tmp/_t.png"] + ([crop] if crop else []), check=True)
    img = base64.b64encode(open("/tmp/_t.png", "rb").read()).decode()
    body = {"model": REPO, "max_tokens": 4096, "temperature": 0, "messages": [{"role": "user", "content": [
        {"type": "image_url", "image_url": {"url": "data:image/png;base64," + img}}, {"type": "text", "text": "OCR:"}]}]}
    r = json.load(urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions",
        json.dumps(body).encode(), {"Content-Type": "application/json"}), timeout=600))
    return r["choices"][0]["message"]["content"]

def score(text):
    got = {norm(l) for l in text.splitlines()}
    return len(want & got)

quads = ["0,0,0.5,0.5", "0.5,0,0.5,0.5", "0,0.5,0.5,0.5", "0.5,0.5,0.5,0.5"]
print(f"ground-truth numbered lines: {len(want)}")
for dpi in (150, 200, 300):
    print(f"full page  {dpi} dpi: {score(ocr(dpi))}/{len(want)}")
for dpi in (150, 200):
    print(f"4 quadrants {dpi} dpi: {score(chr(10).join(ocr(dpi, q) for q in quads))}/{len(want)}")
