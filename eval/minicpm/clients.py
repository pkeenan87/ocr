"""Localhost clients for MiniCPM-OCR 1.0 and MiniCPM-OCR Jev.

MiniCPM-OCR 1.0 is an OpenAI-compatible chat completion: the image, then the prompt
``Text Recognition:``. That is the page-level fallback. The vendor's document pipeline
crops with PP-DocLayoutV3 and picks the prompt from the region (table, formula, seal,
chart). ``--minicpm-mode sdk`` shells out to ``minicpmvocr parse`` for that path.

MiniCPM-OCR Jev is not a chat model. The render+score service takes
``POST /v1/score``'s sibling ``POST /v1/rm/score`` with ``input_form: ternary``,
a data-URL crop, the candidate text and ``category``. Only ``verdict == scored``
is a model score.
"""
import base64
import io
import json
import shutil
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

LOCAL_HOSTS = {"127.0.0.1", "localhost", "::1"}
PROMPT = "Text Recognition:"


class RemoteURL(SystemExit):
    pass


def require_local(url):
    """The harness never sends a page off the machine."""
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme not in ("http", "https") or parsed.hostname not in LOCAL_HOSTS:
        raise RemoteURL(
            f"refusing {url}: MiniCPM servers must be on 127.0.0.1, localhost or ::1"
        )
    return url


def _request(url, payload=None, timeout=900):
    require_local(url)
    data = None if payload is None else json.dumps(payload).encode()
    headers = {"Content-Type": "application/json"} if payload is not None else {}
    req = urllib.request.Request(url, data=data, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = resp.read()
    except urllib.error.HTTPError as exc:
        detail = exc.read()[:300].decode("utf-8", "replace")
        raise RuntimeError(f"HTTP {exc.code} from {url}: {detail}") from exc
    return json.loads(body) if body else {}


def png_bytes(image):
    buf = io.BytesIO()
    image.convert("RGB").save(buf, "PNG")
    return buf.getvalue()


def data_url(png):
    return "data:image/png;base64," + base64.b64encode(png).decode("ascii")


def render_document(path, dpi):
    """Yield (page_number, PIL image) for a PDF or a single image. Page numbers start at 1."""
    path = Path(path)
    if path.suffix.lower() == ".pdf":
        import pypdfium2 as pdfium
        pdf = pdfium.PdfDocument(str(path))
        try:
            scale = dpi / 72.0
            for i in range(len(pdf)):
                pil = pdf[i].render(scale=scale).to_pil().convert("RGB")
                yield i + 1, pil
        finally:
            pdf.close()
        return
    from PIL import Image
    with Image.open(path) as im:
        yield 1, im.convert("RGB")


def _content_text(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(part.get("text") or "" for part in content if isinstance(part, dict))
    return ""


def strip_think(text):
    import re
    text = re.sub(r"<think>.*?</think>", "", text or "", flags=re.S)
    text = re.sub(r"^Text Recognition:\s*", "", text.strip())
    return text.strip()


def chat_body(model, png, max_tokens, backend):
    body = {
        "model": model,
        "temperature": 0.01,
        "top_p": 0.001,
        "max_tokens": max_tokens,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "image_url", "image_url": {"url": data_url(png)}},
                {"type": "text", "text": PROMPT},
            ],
        }],
    }
    if backend == "vllm":
        # Vendor vLLM guide. llama-server rejects some of these, so they are opt-in.
        body.update({
            "top_k": 1,
            "repetition_penalty": 1.0,
            "chat_template_kwargs": {"enable_thinking": False},
            "mm_processor_kwargs": {"downsample_mode": "4x", "max_slice_nums": 9},
        })
    return body


def recognize_pages(path, base_url, model, dpi, max_tokens, backend, timeout):
    """Page mode. Returns markdown with ``<!-- page N -->`` markers and per-page timings."""
    root = base_url.rstrip("/")
    if not root.endswith("/v1"):
        root += "/v1"
    url = root + "/chat/completions"
    require_local(url)
    parts, records = [], []
    t_file = time.perf_counter()
    for number, image in render_document(path, dpi):
        t0 = time.perf_counter()
        rec = {"page": number, "finish_reason": None, "truncated": False, "error": None, "chars": 0}
        try:
            payload = chat_body(model, png_bytes(image), max_tokens, backend)
            data = _request(url, payload, timeout=timeout)
            choice = (data.get("choices") or [{}])[0]
            text = strip_think(_content_text((choice.get("message") or {}).get("content")))
            rec["finish_reason"] = choice.get("finish_reason")
            rec["truncated"] = rec["finish_reason"] == "length"
            rec["otsl"] = "<otsl>" in text.lower()
        except (RuntimeError, OSError, ValueError, KeyError) as exc:
            text = ""
            rec["error"] = str(exc)[:300]
        rec["wall_s"] = round(time.perf_counter() - t0, 4)
        rec["chars"] = len(text)
        parts.append(f"<!-- page {number} -->\n\n{text}\n")
        records.append(rec)
    wall = time.perf_counter() - t_file
    return "\n".join(parts), records, wall


def recognize_sdk(path, dest, server_url, model, sdk_bin, extra):
    """Vendor pipeline: layout, per-region prompts, Markdown assembly. One process per file."""
    root = server_url.rstrip("/")
    if root.endswith("/v1"):
        root = root[:-3]
    require_local(root)
    bin_path = sdk_bin or shutil.which("minicpmvocr")
    if not bin_path:
        raise RuntimeError("minicpmvocr is not on PATH; install the SDK or use --minicpm-mode page")
    dest.parent.mkdir(parents=True, exist_ok=True)
    cmd = [bin_path, "parse", str(path), "-o", str(dest), "--server-url", root, "--model", model, *extra]
    t0 = time.perf_counter()
    proc = subprocess.run(cmd, capture_output=True, text=True)
    wall = time.perf_counter() - t0
    if proc.returncode != 0:
        tail = (proc.stderr or proc.stdout or "")[-300:]
        raise RuntimeError(f"minicpmvocr exited {proc.returncode}: {tail}")
    text = dest.read_text(encoding="utf-8") if dest.exists() else ""
    return text, wall


def score_regions(jev_url, items, timeout, protocol):
    """Score candidate regions. ``protocol`` is ``rm`` (render+score) or ``score`` (you rendered)."""
    root = jev_url.rstrip("/")
    require_local(root)
    if protocol == "score":
        url = root + "/v1/score"
        payload = {"items": [{
            "original_image": item["image"].split(",", 1)[-1],
            "render_image": item["render_image"].split(",", 1)[-1],
            "ocr_text": item["ocr_text"],
        } for item in items]}
    else:
        url = root + "/v1/rm/score"
        payload = {"input_form": "ternary", "items": [{
            "case_id": item["case_id"],
            "image": item["image"],
            "ocr_text": item["ocr_text"],
            "category": item["category"],
        } for item in items]}
    data = _request(url, payload, timeout=timeout)
    return data.get("results") or []


def render_candidate(text, width=800):
    """Approximate text render for ``/v1/score`` when the render+score service is absent.

    This is Pillow drawing glyphs, not the vendor's Chromium/LaTeX renderer. Formula
    scores from this path are not comparable to a ``verdict: scored`` run that typeset
    the candidate. Prefer ``--jev-protocol rm``.
    """
    from PIL import Image, ImageDraw
    lines = (text or "").splitlines() or [""]
    height = max(40, 8 + 18 * len(lines[:80]))
    image = Image.new("RGB", (width, height), "white")
    draw = ImageDraw.Draw(image)
    y = 4
    for ln in lines[:80]:
        draw.text((4, y), ln[:200], fill="black")
        y += 18
    return png_bytes(image)
