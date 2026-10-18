#!/usr/bin/env python3
"""Write THIRD_PARTY_NOTICES.md for a binary release: the license of every Rust crate compiled
into `ocr` for one target, with the license files each crate ships, plus the components that
`ocr server setup` downloads at run time (not bundled).

    python3 scripts/third_party_notices.py --target x86_64-unknown-linux-gnu > THIRD_PARTY_NOTICES.md
"""
import argparse, json, re, subprocess, sys
from pathlib import Path

DOWNLOADED = """\
## Downloaded by `ocr server setup` (not bundled)

These are fetched at setup time, checked against a pinned sha256, and used under their own licenses:

- **llama.cpp** `llama-server` (release b11510), MIT: https://github.com/ggml-org/llama.cpp
- **pdfium** (chromium/7881, from https://github.com/bblanchon/pdfium-binaries), BSD-3-Clause and Apache-2.0
- **Model weights** from Hugging Face, each under its model's license: PaddleOCR-VL (Apache-2.0),
  olmOCR 2 (Apache-2.0), dots.ocr / dots.mocr (see the model card), indra-ocr (modified
  OpenRAIL-M, restricted commercial use; read https://huggingface.co/datalab-to/indra-ocr)
- **indra-ocr** Python package (0.3.1) and its dependencies, installed by `uv`
"""

LICENSE_FILE = re.compile(r"^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)", re.I)


def runtime_packages(target):
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", target],
        check=True, capture_output=True, text=True).stdout)
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root, seen, stack = meta["resolve"]["root"], set(), [meta["resolve"]["root"]]
    while stack:
        i = stack.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            if any(k["kind"] in (None, "normal") for k in d["dep_kinds"]):
                stack.append(d["pkg"])
    seen.discard(root)
    pkgs = [p for p in meta["packages"] if p["id"] in seen]
    return sorted(pkgs, key=lambda p: (p["name"], p["version"]))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", required=True)
    args = ap.parse_args()
    out = ["# Third-party notices\n",
           f"`ocr` is MIT-licensed (see LICENSE). The release binary for `{args.target}` contains "
           "the Rust crates below, each under its own license; their license files follow.\n",
           DOWNLOADED, "## Rust crates in the binary\n", "| Crate | Version | License |", "|---|---|---|"]
    pkgs = runtime_packages(args.target)
    for p in pkgs:
        out.append(f"| {p['name']} | {p['version']} | {p['license'] or p.get('license_file') or '?'} |")
    out.append("\n## License texts\n")
    missing, shown = [], {}
    for p in pkgs:
        files = sorted(f for f in Path(p["manifest_path"]).parent.iterdir()
                       if f.is_file() and LICENSE_FILE.match(f.name))
        if not files:
            missing.append(p["name"])
            continue
        out.append(f"### {p['name']} {p['version']}\n")
        for f in files:
            text = f.read_text(errors="replace").strip()
            key = re.sub(r"\s+", " ", text)
            if key in shown:
                out.append(f"{f.name}: same text as {shown[key]}.\n")
            else:
                shown[key] = f"{p['name']} {p['version']} ({f.name})"
                out.append(f"{f.name}:\n\n```text\n{text}\n```\n")
    if missing:
        out.append("### Crates without a license file in their package\n")
        out.append("These are distributed under the license named in the table above: " + ", ".join(missing) + ".\n")
    sys.stdout.write("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
