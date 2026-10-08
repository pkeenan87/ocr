#!/usr/bin/python3
"""Stand-in for mlx_vlm.server in tests. Same argv; parses --port and --model, ignores the rest.

Behaviour is chosen by env FAKE_MODE: ok, slow_ready, never_ready, crash, loop, length,
error5xx, sleep.
"""
import base64
import json
import os
import re
import struct
import sys
import time
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODE = os.environ.get("FAKE_MODE", "ok")
STARTED = time.time()
PORT = 8080
MODEL = ""

args = sys.argv[1:]
for i, a in enumerate(args):
    if a == "--port":
        PORT = int(args[i + 1])
    elif a == "--model":
        MODEL = args[i + 1]


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
        if MODE in ("never_ready", "error5xx"):
            return self.send(500, {"error": "boom"})
        if MODE == "sleep":
            time.sleep(30)
        text = json.dumps(req)
        for m in re.finditer(r"data:image/png;base64,([A-Za-z0-9+/=]+)", text):
            if not png_ok(base64.b64decode(m.group(1))):
                return self.send(400, {"error": "bad png"})
        content, reason = "fake ocr text", "stop"
        if MODE == "loop":
            content = "the same line again\n" * 200
        elif MODE == "length":
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
