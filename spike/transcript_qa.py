import base64, json, os, re, signal, subprocess, sys, time, urllib.request
repo, port, prompt = sys.argv[1], sys.argv[2], sys.argv[3]
srv = subprocess.Popen(["spike/.venv/bin/mlx_vlm.server", "--host", "127.0.0.1", "--port", port, "--model", repo, "--max-num-seqs", "1"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
def chat(path, mt=3000):
    img = base64.b64encode(open(path, "rb").read()).decode()
    body = {"model": repo, "max_tokens": mt, "temperature": 0, "messages": [{"role": "user", "content": [
        {"type": "image_url", "image_url": {"url": "data:image/png;base64," + img}}, {"type": "text", "text": prompt}]}]}
    r = urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions", json.dumps(body).encode(), {"Content-Type": "application/json"}), timeout=900)
    return json.load(r)["choices"][0]["message"]["content"]
try:
    for _ in range(120):
        try: urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2); break
        except Exception: time.sleep(1)
    t = chat("spike/out/transcript_full.png")
    ok = sum(1 for ln in range(1, 26) if re.search(rf"\b{ln}\s+{'Q' if ln % 2 else 'A'}\. Line {ln} of the testimony", t))
    print(f"full transcript: {ok}/25 lines with number + Q/A + text exact")
    tot = 0
    for k in range(1, 5):
        t = chat(f"spike/out/quad{k}.png")
        tot += sum(1 for ln in range(1, 26) if re.search(rf"\b{ln}\s+Q\. Question {k}\.{ln} about the meeting", t))
    print(f"4-up quadrant crops: {tot}/100 exact")
    t = chat("spike/out/four_up_300.png", 6000)
    tot = sum(1 for k in range(1, 5) for ln in range(1, 26) if re.search(rf"\b{ln}\s+Q\. Question {k}\.{ln} about the meeting", t))
    print(f"4-up whole page: {tot}/100 exact")
finally:
    os.killpg(srv.pid, signal.SIGTERM)
