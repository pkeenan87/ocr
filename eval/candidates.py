#!/usr/bin/env python3
"""Shared helpers for the raw-harness candidate evaluation: load entries from
eval/candidates.json, render a page, build and send an OpenAI chat request,
list the server's models, convert the reply to Markdown, and assemble
page-marked Markdown. `python3 eval/candidates.py --self-test` checks it."""
import argparse
import base64
import io
import json
import re
import sys
import time
import urllib.request
import urllib.error
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONFIG = ROOT / "eval/candidates.json"
MODELS = ROOT / "eval/out/candidates/models"

REQUIRED = ("repo", "revision", "model", "mmproj", "prompt", "text_first", "render",
            "max_tokens", "convert", "thinking_off", "ctx", "license", "role")


def all_entries() -> dict:
    with open(CONFIG) as f:
        return json.load(f)


def load(name: str) -> dict:
    cfg = all_entries()
    if name not in cfg:
        raise SystemExit(f"unknown candidate {name!r}; known: {', '.join(sorted(cfg))}")
    return dict(cfg[name], name=name)


def prompt_of(c: dict) -> str:
    if c["prompt"] == "@olmocr_v4":
        sys.path.insert(0, str(ROOT / "spike/upstream"))
        from allenai_olmocr import build_no_anchoring_v4_yaml_prompt
        return build_no_anchoring_v4_yaml_prompt()
    return c["prompt"]


def render(page, spec: dict) -> bytes:
    w, h = page.get_size()
    scale = spec["dpi"] / 72 if "dpi" in spec else spec["side"] / max(w, h)
    buf = io.BytesIO()
    page.render(scale=scale).to_pil().convert("RGB").save(buf, "PNG")
    return buf.getvalue()


def build_body(c: dict, png: bytes, prompt: str) -> dict:
    image = {"type": "image_url", "image_url": {"url": "data:image/png;base64," + base64.b64encode(png).decode()}}
    if prompt == "":
        content = [image]
    else:
        text = {"type": "text", "text": prompt}
        content = [text, image] if c["text_first"] else [image, text]
    body = {"model": c["name"], "temperature": 0, "max_tokens": c["max_tokens"],
            "messages": [{"role": "user", "content": content}]}
    if c["thinking_off"]:
        body["chat_template_kwargs"] = {"enable_thinking": False}
    return body


def ask(port: int, c: dict, png: bytes, prompt: str, timeout: int = 900) -> dict:
    t0 = time.time()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/v1/chat/completions",
            data=json.dumps(build_body(c, png, prompt)).encode(),
            headers={"Content-Type": "application/json"},
            method="POST")
        with urllib.request.urlopen(req, timeout=timeout) as r:
            data = json.load(r)
        choice = data["choices"][0]
        return {"reply": choice["message"]["content"] or "",
                "finish_reason": choice.get("finish_reason"),
                "completion_tokens": (data.get("usage") or {}).get("completion_tokens"),
                "wall_s": round(time.time() - t0, 2),
                "error": None}
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, KeyError, IndexError) as e:
        return {"reply": "", "finish_reason": None, "completion_tokens": None,
                "wall_s": round(time.time() - t0, 2),
                "error": str(e)}


def server_model_ids(port: int) -> list:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/v1/models", timeout=10) as r:
            data = json.load(r)
        return [m.get("id", "") for m in data.get("data", [])]
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError):
        return []


def convert(c: dict, reply: str) -> str:
    reply = re.sub(r"^\s*<think>.*?</think>\s*", "", reply, flags=re.S)
    kind = c["convert"]
    if kind == "raw":
        return reply.strip()
    if kind == "olmocr_yaml":
        m = re.match(r"\s*---\n.*?\n---\n?", reply, re.S)
        return (reply[m.end():] if m else reply).strip()
    if kind == "ovis_markdown":
        return "\n\n".join(b for b in reply.strip().split("\n\n")
                           if not b.strip().startswith('<img src="images/bbox_'))
    raise ValueError(f"unknown convert {kind!r}")


def page_markdown(pages: list) -> str:
    out = []
    for number, text, error in pages:
        if error:
            out.append(f"<!-- page {number}: OCR failed: {error.replace('-->', '- -')} -->\n\n")
        else:
            out.append(f"<!-- page {number} -->\n\n{text}\n\n")
    return "".join(out)


def self_test():
    assert sorted(all_entries()) == sorted(["paddle-raw", "olmocr-raw", "dots-ocr-raw",
                                            "dots-mocr", "glm-ocr", "ovisocr3", "lighton-4b"])
    for name, c in sorted(all_entries().items()):
        for k in REQUIRED:
            assert k in c, (name, k)
        for part in ("model", "mmproj"):
            assert re.fullmatch(r"[0-9a-f]{64}", c[part]["sha256"]), (name, part)
            assert isinstance(c[part]["size"], int), (name, part)
        assert c["convert"] in ("raw", "olmocr_yaml", "ovis_markdown"), name
    assert convert({"convert": "raw"}, "<think>x</think>\nhello ") == "hello"
    assert convert({"convert": "olmocr_yaml"}, "---\nprimary_language: en\n---\nBody") == "Body"
    assert convert({"convert": "ovis_markdown"}, 'a\n\n<img src="images/bbox_1_2_3_4.jpg" />\n\nb') == "a\n\nb"
    assert page_markdown([(2, "x", None), (3, "", "boom")]) == \
        "<!-- page 2 -->\n\nx\n\n<!-- page 3: OCR failed: boom -->\n\n"
    b = build_body(load("lighton-4b"), b"x", "")
    assert len(b["messages"][0]["content"]) == 1 and "chat_template_kwargs" in b
    assert build_body(load("olmocr-raw"), b"x", "p")["messages"][0]["content"][0]["type"] == "text"
    assert ask(1, load("glm-ocr"), b"x", "p", timeout=2)["error"] is not None
    assert server_model_ids(1) == []  # nothing listens on port 1
    print("self-test ok")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        self_test()
    else:
        for name in sorted(all_entries()):
            print(name)
