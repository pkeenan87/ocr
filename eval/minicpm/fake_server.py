"""Local stand-in for MiniCPM-OCR 1.0 chat completions and MiniCPM-OCR Jev /v1/rm/score.

The process binds 127.0.0.1 on an ephemeral port. It records whether the OCR call
sent the image before the text ``Text Recognition:`` and whether Jev was called
with ``input_form: ternary``. It does not store page text. A candidate is scored
low only when it contains one of the planted misreads.
"""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOW_MARKS = ("99,999.00", "88,888.00")
_LOCK = threading.Lock()
STATE = {
    "pages": [],
    "index": 0,
    "image_before_text": 0,
    "bad_prompt": 0,
    "ternary": 0,
    "score_calls": 0,
    "chat_calls": 0,
}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        return

    def _send(self, code, payload):
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path in ("/health", "/healthz", "/v1/models"):
            self._send(200, {"status": "ok", "data": [{"id": "minicpm-ocr"}]})
            return
        if self.path == "/__control":
            with _LOCK:
                snap = {k: v for k, v in STATE.items() if k != "pages"}
                snap["pages"] = len(STATE["pages"])
            self._send(200, snap)
            return
        self._send(404, {"error": "not found"})

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b"{}"
        try:
            payload = json.loads(raw.decode() or "{}")
        except json.JSONDecodeError:
            self._send(400, {"error": "bad json"})
            return
        if self.path == "/__control":
            with _LOCK:
                if "pages" in payload:
                    STATE["pages"] = list(payload["pages"])
                    STATE["index"] = 0
            self._send(200, {"ok": True})
            return
        if self.path.endswith("/chat/completions"):
            self._chat(payload)
            return
        if self.path.endswith("/v1/rm/score"):
            self._rm(payload)
            return
        if self.path.endswith("/v1/score"):
            self._score(payload)
            return
        self._send(404, {"error": "not found"})

    def _chat(self, payload):
        messages = payload.get("messages") or []
        content = (messages[0].get("content") if messages else None) or []
        ok = (
            isinstance(content, list)
            and len(content) >= 2
            and content[0].get("type") == "image_url"
            and content[1].get("type") == "text"
            and content[1].get("text") == "Text Recognition:"
        )
        with _LOCK:
            STATE["chat_calls"] += 1
            if ok:
                STATE["image_before_text"] += 1
            else:
                STATE["bad_prompt"] += 1
            pages = STATE["pages"]
            idx = STATE["index"]
            text = pages[idx % len(pages)] if pages else ""
            STATE["index"] = idx + 1
        if not ok:
            self._send(400, {"error": "expected an image followed by the text Text Recognition:"})
            return
        self._send(200, {"choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]})

    def _prob(self, text):
        return 0.05 if any(mark in (text or "") for mark in LOW_MARKS) else 0.97

    def _rm(self, payload):
        with _LOCK:
            STATE["score_calls"] += 1
            if payload.get("input_form") == "ternary":
                STATE["ternary"] += 1
        if payload.get("input_form") != "ternary":
            self._send(422, {"error": "input_form must be ternary"})
            return
        results = []
        for item in payload.get("items") or []:
            prob = self._prob(item.get("ocr_text"))
            results.append({
                "case_id": item.get("case_id"),
                "category": item.get("category") or "text",
                "verdict": "scored",
                "prob_yes": prob,
                "prob_no": round(1 - prob, 4),
                "detail": "",
            })
        self._send(200, {"results": results, "summary": {"total": len(results), "scored": len(results)}})

    def _score(self, payload):
        with _LOCK:
            STATE["score_calls"] += 1
        results = [{"prob_1": self._prob(item.get("ocr_text")), "error": None} for item in payload.get("items") or []]
        self._send(200, {"results": results})


def serve(pages):
    """Start a daemon server. Returns the ``ThreadingHTTPServer`` (call ``shutdown()``)."""
    with _LOCK:
        STATE["pages"] = list(pages)
        STATE["index"] = 0
        STATE["image_before_text"] = 0
        STATE["bad_prompt"] = 0
        STATE["ternary"] = 0
        STATE["score_calls"] = 0
        STATE["chat_calls"] = 0
    httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    return httpd
