#!/usr/bin/env python3
"""Fetch a candidate's pinned GGUF model + mmproj and start llama-server.

Usage:
    python3 eval/candidate_serve.py <name> [--port 8095] [--gpu 0] [--fetch-only] [--dry-run]

Downloads the model and mmproj of a candidates.json entry (sha256-checked,
skipped when already verified) and replaces itself with the installed
llama-server on that port and GPU. The server runs in the foreground
(Ctrl-C stops it). Models live under eval/out/candidates/models/ (gitignored).
"""
import argparse
import hashlib
import os
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import candidates


def fetch_file(c: dict, f: dict, dry: bool) -> Path:
    dest = candidates.MODELS / c["name"] / f["name"]
    stamp = dest.with_name(dest.name + ".sha256")
    url = f"https://huggingface.co/{c['repo']}/resolve/{c['revision']}/{f['name']}"
    if (dest.exists() and dest.stat().st_size == f["size"]
            and stamp.exists() and stamp.read_text().strip() == f["sha256"]):
        return dest
    if dry:
        print(f"would download {url} -> {dest}")
        return dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_name(dest.name + ".part")
    h = hashlib.sha256()
    got = 0
    last = -1
    with urllib.request.urlopen(url, timeout=60) as r, open(part, "wb") as out:
        while True:
            chunk = r.read(1 << 20)
            if not chunk:
                break
            out.write(chunk)
            h.update(chunk)
            got += len(chunk)
            pct = got * 100 // f["size"]
            if pct % 5 == 0 and pct != last:
                last = pct
                print(f"{f['name']}: {pct}%")
    if got != f["size"] or h.hexdigest() != f["sha256"]:
        part.unlink()
        raise SystemExit(f"{f['name']}: sha256 mismatch (got {h.hexdigest()}, want {f['sha256']})")
    part.rename(dest)
    stamp.write_text(f["sha256"] + "\n")
    return dest


def state_dir() -> Path:
    return Path(os.environ.get("OCR_STATE_DIR") or Path.home() / ".local/share/ocr")


def find_server() -> tuple:
    base = state_dir() / "llama.cpp"
    found = sorted(base.glob("*/**/llama-server"))
    if not found:
        return (None, [])
    server = next((p for p in found if "cuda" in str(p)), found[0])
    root = base / server.relative_to(base).parts[0]
    libs = sorted({p.parent for p in root.rglob("*") if p.is_file() and ".so" in p.name})
    return (server, libs)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("name")
    ap.add_argument("--port", type=int, default=8095)
    ap.add_argument("--gpu", type=str, default="0")
    ap.add_argument("--fetch-only", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    c = candidates.load(a.name)
    m = fetch_file(c, c["model"], a.dry_run)
    p = fetch_file(c, c["mmproj"], a.dry_run)
    if a.fetch_only:
        print(m)
        print(p)
        return
    server, libs = find_server()
    argv = [str(server) if server else "<llama-server>", "--model", str(m), "--mmproj", str(p),
            "--host", "127.0.0.1", "--port", str(a.port), "--parallel", "1",
            "--ctx-size", str(c.get("ctx", 32768)), "--n-gpu-layers", "999"]
    print(" ".join(argv))
    if a.dry_run:
        return
    if server is None:
        raise SystemExit(f"llama-server not found under {state_dir()}/llama.cpp; run `ocr server setup --model paddleocr-vl` first")
    env = dict(os.environ)
    env["CUDA_VISIBLE_DEVICES"] = a.gpu
    env["LD_LIBRARY_PATH"] = ":".join([*map(str, libs), os.environ.get("LD_LIBRARY_PATH", "")]).strip(":")
    os.execve(argv[0], argv, env)


if __name__ == "__main__":
    main()
