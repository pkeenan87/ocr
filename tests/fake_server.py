#!/usr/bin/python3
"""Stand-in for mlx_vlm.server in tests. Same argv; parses --port and --model, ignores the rest.

Behaviour is chosen by env FAKE_MODE: ok, slow_ready, never_ready, crash, loop, length,
error5xx, sleep, empty, longtable, and per engine: rotate and nofront (olmOCR), truncated (dots). The
request modes skip the readiness probe (max_tokens <= 8). FAKE_FAIL_FIRST=N limits them to the
first N real requests; FAKE_SLEEP_SECS sets how long `sleep` blocks (default 30).

The reply format follows the prompt, so no engine setting is needed: `Attached is one page...`
is olmOCR (YAML front matter, text, a figure, an HTML table), `Please output the layout...` is
dots `prompt_layout_all_en` (a JSON array of {bbox, category, text}), `Extract the text content`
is dots `prompt_ocr` (plain text), `Table Recognition:` is OTSL, anything else is `OCR:`.
Modes: `loop` makes the text (olmOCR prose, a dots Text field, plain text) loop; `length` sets
finish_reason length (a dots array is cut off too); `empty` returns an empty reply; `rotate`
makes olmOCR report is_rotation_valid False with rotation_correction 90; `nofront` drops the
olmOCR front matter; `longtable` adds a table of 80 identical rows (olmOCR text, dots Table
element), which must not be mistaken for a loop; `truncated` cuts the dots JSON in the middle of an element with finish
stop.

Every real request is logged as one JSON line to stderr (`fake_server: REQ {...}`, so into the
server log) and, if FAKE_LOG names a file, appended there too. With FAKE_LOG the request number
`n` counts the lines in that file, so it survives server restarts:
`{"n": 3, "prompt": "OCR:", "max_tokens": 4096, "repetition_penalty": null, "w": 1700, "h": 2200,
"text_first": false}` (`text_first`: the text part comes before the image part in the message).
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
        text_first = False
        for msg in req.get("messages", []):
            if isinstance(msg.get("content"), list):
                kinds = [part.get("type") for part in msg["content"]]
                text_first = kinds[:1] == ["text"]
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
                                   "w": w, "h": h, "text_first": text_first})
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
        content, reason = reply(prompt, w, h, active)
        self.send(200, {
            "model": MODEL,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": content},
                         "finish_reason": reason}],
        })


OLM_FRONT = ("---\nprimary_language: en\nis_rotation_valid: {valid}\nrotation_correction: {rot}\n"
             "is_table: True\nis_diagram: False\n---\n")
OLM_TABLE = ("<table>\n<tr><th>Invoice</th><th>Date</th><th>Amount</th></tr>\n"
             "<tr><td>INV-1001</td><td>2026-01-05</td><td>$48,250.00</td></tr>\n"
             "<tr><td>INV-1002</td><td>2026-02-11</td><td>$51,900.00</td></tr>\n</table>")
LOOP = "the same line again\n" * 200
LONG_TABLE = ("<table>\n<tr><th>Item</th><th>Cost</th><th>Note</th></tr>\n"
              + "<tr><td>N/A</td><td>$0.00</td><td>Not applicable</td></tr>\n" * 80 + "</table>")


def dots_elements(w, h):
    return [
        {"bbox": [10, 10, 90, 30], "category": "Page-header", "text": "FAKE RUNNING HEADER"},
        {"bbox": [10, 40, 90, 80], "category": "Title", "text": f"FAKE TITLE {w}x{h}"},
        {"bbox": [10, 90, 90, 110], "category": "Section-header", "text": "Section A"},
        {"bbox": [10, 120, 90, 160], "category": "Text", "text": f"fake dots text {w}x{h}"},
        {"bbox": [10, 170, 90, 190], "category": "List-item", "text": "first item"},
        {"bbox": [10, 195, 90, 215], "category": "List-item", "text": "second item"},
        {"bbox": [10, 220, 90, 300], "category": "Table",
         "text": OLM_TABLE.replace("\n", "")},
        {"bbox": [10, 310, 90, 340], "category": "Formula", "text": "E = mc^{2}"},
        {"bbox": [10, 350, 90, 450], "category": "Picture"},
        {"bbox": [10, 455, 90, 470], "category": "Caption", "text": "Figure 1. A fake picture"},
        {"bbox": [10, 480, 90, 490], "category": "Footnote", "text": "1 A fake footnote."},
        {"bbox": [10, 495, 90, 500], "category": "Page-footer", "text": "FAKE RUNNING FOOTER"},
    ]


def reply(prompt, w, h, active):
    """(content, finish_reason) for a real request; `active` turns the failure mode on."""
    mode = MODE if active else "ok"
    reason = "length" if mode == "length" else "stop"
    if mode == "empty":
        return "", reason
    if prompt.startswith("Attached is one page"):
        valid, rot = ("False", 90) if mode == "rotate" else ("True", 0)
        front = "" if mode == "nofront" else OLM_FRONT.format(valid=valid, rot=rot)
        if mode == "loop":
            return front + LOOP, reason
        table = LONG_TABLE if mode == "longtable" else OLM_TABLE
        return (front + f"fake olmocr text {w}x{h}\n\n![A bar chart](page_10_20_300_400.png)\n\n"
                + table + "\n\nClosing line"), reason
    if prompt.startswith("Please output the layout"):
        elements = dots_elements(w, h)
        if mode == "loop":
            elements[3]["text"] = LOOP
        if mode == "longtable":
            elements[6]["text"] = LONG_TABLE.replace("\n", "")
        text = json.dumps(elements)
        if mode in ("length", "truncated"):
            text = text[:int(len(text) * 0.7)]
        return text, reason
    if prompt.startswith("Extract the text content"):
        return (LOOP if mode == "loop" else f"fake dots plain text {w}x{h}"), reason
    if prompt.startswith("Table Recognition"):
        return ("<fcel>Invoice<fcel>Date<fcel>Amount<nl>"
                "<fcel>INV-1001<fcel>2026-01-05<fcel>$48,250.00<nl>"
                "<fcel>INV-1002<fcel>2026-02-11<fcel>$51,900.00<nl>"), reason
    return (LOOP if mode == "loop" else f"fake ocr text {w}x{h}"), reason


if MODE == "crash":
    sys.stderr.write("fake_server: crashing on purpose\n")
    sys.exit(1)

ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
