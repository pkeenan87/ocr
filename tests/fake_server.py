#!/usr/bin/python3
"""Stand-in for mlx_vlm.server in tests. Same argv; parses --port and --model, ignores the rest.

Behaviour is chosen by env FAKE_MODE: ok, slow_ready, never_ready, crash, loop, length,
error5xx, sleep. The request modes (loop, length, error5xx, sleep) skip the readiness probe
(max_tokens <= 8). FAKE_FAIL_FIRST=N limits them to the first N real requests; FAKE_SLEEP_SECS
sets how long `sleep` blocks (default 30).

Every real request is logged as one JSON line to stderr (`fake_server: REQ {...}`, so into the
server log) and, if FAKE_LOG names a file, appended there too. With FAKE_LOG the request number
`n` counts the lines in that file, so it survives server restarts:
`{"n": 3, "prompt": "OCR:", "max_tokens": 4096, "repetition_penalty": null, "w": 1700, "h": 2200}`.
OCR replies are `fake ocr text WxH` (the size of the image received, so crops differ from
full-page requests); `Table Recognition:` replies are OTSL.
"""
import base64
import json
import os
import re
import struct
import sys
import threading
import time
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODE = os.environ.get("FAKE_MODE", "ok")
STARTED = time.time()
PORT = 8080
MODEL = ""
LOG = os.environ.get("FAKE_LOG")  # optional extra log file, appended to across restarts
REQUESTS = 0
LOCK = threading.Lock()

args = sys.argv[1:]
for i, a in enumerate(args):
    if a == "--port":
        PORT = int(args[i + 1])
    elif a == "--model":
        MODEL = args[i + 1]


def png_size(data):
    """(width, height) from the PNG IHDR chunk."""
    return struct.unpack(">II", data[16:24])


def png_ok(data):
    """True if data is a decodable PNG (signature, chunk CRCs, zlib stream)."""
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        return False
    pos, idat = 8, b""
    while pos < len(data):
        (n,) = struct.unpack(">I", data[pos:pos + 4])
        kind, body = data[pos + 4:pos + 8], data[pos + 8:pos + 8 + n]
        (crc,) = struct.unpack(">I", data[pos + 8 + n:pos + 12 + n])
        if zlib.crc32(kind + body) != crc:
            return False
        if kind == b"IDAT":
            idat += body
        pos += 12 + n
    try:
        zlib.decompress(idat)
    except zlib.error:
        return False
    return True


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *a):
        sys.stderr.write("fake_server: " + fmt % a + "\n")
        sys.stderr.flush()

    def send(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/health":
            if MODE == "slow_ready" and time.time() - STARTED < 2:
                return self.send(503, {"status": "loading"})
            return self.send(200, {"status": "healthy", "model": MODEL})
        self.send(404, {})

    def do_POST(self):
        if self.path != "/v1/chat/completions":
            return self.send(404, {})
        n = int(self.headers.get("Content-Length", 0))
        req = json.loads(self.rfile.read(n) or b"{}")
        if MODE == "never_ready":
            return self.send(500, {"error": "boom"})
        text = json.dumps(req)
        images = [base64.b64decode(m.group(1))
                  for m in re.finditer(r"data:image/png;base64,([A-Za-z0-9+/=]+)", text)]
        for img in images:
            if not png_ok(img):
                return self.send(400, {"error": "bad png"})
        prompt = ""
        for msg in req.get("messages", []):
            if isinstance(msg.get("content"), list):
                for part in msg["content"]:
                    if part.get("type") == "text":
                        prompt = part["text"]
        max_tokens = req.get("max_tokens", 0)
        probe = max_tokens <= 8
        n = 0
        if not probe:
            w, h = png_size(images[0]) if images else (0, 0)
            with LOCK:
                global REQUESTS
                REQUESTS += 1
                n = REQUESTS
                if LOG:
                    n = sum(1 for _ in open(LOG)) + 1 if os.path.exists(LOG) else 1
                line = json.dumps({"n": n, "prompt": prompt, "max_tokens": max_tokens,
                                   "repetition_penalty": req.get("repetition_penalty"),
                                   "w": w, "h": h})
                sys.stderr.write("fake_server: REQ " + line + "\n")
                sys.stderr.flush()
                if LOG:
                    with open(LOG, "a") as f:
                        f.write(line + "\n")
        first = os.environ.get("FAKE_FAIL_FIRST")
        active = not probe and (first is None or n <= int(first))
        if active and MODE == "error5xx":
            return self.send(500, {"error": "boom"})
        if active and MODE == "sleep":
            time.sleep(float(os.environ.get("FAKE_SLEEP_SECS", "30")))
        w, h = png_size(images[0]) if images else (0, 0)
        if prompt.startswith("Table Recognition"):
            content = ("<fcel>Invoice<fcel>Date<fcel>Amount<nl>"
                       "<fcel>INV-1001<fcel>2026-01-05<fcel>$48,250.00<nl>"
                       "<fcel>INV-1002<fcel>2026-02-11<fcel>$51,900.00<nl>")
        else:
            content = f"fake ocr text {w}x{h}"
        reason = "stop"
        if active and MODE == "loop":
            content = "the same line again\n" * 200
        elif active and MODE == "length":
            reason = "length"
        self.send(200, {
            "model": MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": content},
                         "finish_reason": reason}],
        })


if MODE == "crash":
    sys.stderr.write("fake_server: crashing on purpose\n")
    sys.exit(1)

ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
