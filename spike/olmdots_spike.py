"""olmOCR / dots engine spike (docs/spike.md, "olmOCR and dots engine spike").

usage: olmdots_spike.py <olmocr|dots> <port> <repo> <server.log> <out-dir> [scenario...]

Needs a running `ocr server start --model <engine> --port <port>` (one server at a time) and
target/release/examples/{render_png,crop_plan} (the production render path and crop plan).
Scenarios: legal, columns, transcripts, rot90, dpi (dots only).
"""
import base64, json, os, re, subprocess, sys, time, urllib.request

engine, port, repo, log, out = sys.argv[1:6]
scenarios = sys.argv[6:] or ["legal", "columns", "transcripts", "rot90"]
os.makedirs(out, exist_ok=True)
ROOT = os.getcwd()
RENDER = f"{ROOT}/target/release/examples/render_png"
CROPS = f"{ROOT}/target/release/examples/crop_plan"
FIX = f"{ROOT}/spike/fixtures"
OLM_PROMPT = ("Attached is one page of a document that you must process. "
    "Just return the plain text representation of this document as if you were reading it naturally. Convert equations to LateX and tables to HTML.\n"
    "If there are any figures or charts, label them with the following markdown syntax ![Alt text describing the contents of the figure](page_startx_starty_width_height.png)\n"
    "Return your output as markdown, with a front matter section on top specifying values for the primary_language, is_rotation_valid, rotation_correction, is_table, and is_diagram parameters.")
ns = {}
exec(open(f"{ROOT}/spike/upstream/rednote-hilab_dots.mocr.py").read(), ns)
DOTS = ns["dict_promptmode_to_prompt"]
TOKENS = ["3:26-cv-04417", "$48,250.00", "$51,900.00", "$22,375.50", "$122,525.50", "SMITH000123"]


def render(pdf, crop=None, dpi=200, side=None, rot=0):
    dst = f"{out}/_img.png"
    cmd = [RENDER, pdf, "1", str(dpi), dst, crop or "-"] + ([str(side), str(rot)] if side else [])
    subprocess.run(cmd, check=True)
    return open(dst, "rb").read()


def log_lines():
    return [l for l in open(log) if "Request completed" in l]


def chat(png, prompt, max_tokens):
    content = [{"type": "image_url", "image_url": {"url": "data:image/png;base64," + base64.b64encode(png).decode()}},
               {"type": "text", "text": prompt}]
    if engine == "olmocr":
        content.reverse()  # upstream: text first, then the image
    body = {"model": repo, "max_tokens": max_tokens, "temperature": 0,
            "messages": [{"role": "user", "content": content}]}
    n = len(log_lines())
    t = time.time()
    r = json.load(urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions", json.dumps(body).encode(), {"Content-Type": "application/json"}), timeout=1800))
    dt = time.time() - t
    time.sleep(0.3)
    m = re.search(r"prompt_tokens=(\d+) generated_tokens=(\d+) elapsed=([\d.]+)s prefill=([\d.]+) tok/s decode=([\d.]+) tok/s", "".join(log_lines()[n:]))
    stats = dict(zip(["prompt_tokens", "gen_tokens", "server_s", "prefill_tps", "decode_tps"], map(float, m.groups()))) if m else {}
    stats["wall_s"] = round(dt, 1)
    return r["choices"][0]["message"]["content"], r["choices"][0]["finish_reason"], stats


def run(png, kind="main"):
    if engine == "olmocr":
        return chat(png, OLM_PROMPT, 8000)
    if kind == "ocr":
        return chat(png, DOTS["prompt_ocr"], 8192)
    return chat(png, DOTS["prompt_layout_all_en"], 8192)


def plain(content):
    """The text a scorer should look at: dots JSON `text` fields joined, else the raw reply."""
    if engine == "dots":
        try:
            arr = json.loads(re.sub(r"^```(?:json)?|```$", "", content.strip()).strip())
            return "\n".join(e.get("text", "") for e in arr if isinstance(e, dict))
        except Exception:
            pass
    return content


def say(label, content, fin, stats):
    print(f"[{label}] finish={fin} {stats}", flush=True)
    open(f"{out}/{re.sub(r'[^A-Za-z0-9_.-]', '_', label)}.txt", "w").write(content)


def score_lines(text, expected):
    found = [text.find(s) for s in expected if s in text]
    ordered = sum(b > a for a, b in zip(found, found[1:]))
    return len(found), ordered, max(len(found) - 1, 0)


two_col = [f"Left column sentence {i} concerns" for i in range(1, 16)] + [f"Right column sentence {i} concerns" for i in range(1, 16)]
narrow = json.load(open(f"{FIX}/narrow_unequal_seed2.expected.json"))
side = 1288 if engine == "olmocr" else None

if "legal" in scenarios:
    for name in ("legal", "legal_scan"):
        c, fin, st = run(render(f"{FIX}/{name}.pdf", side=side))
        say(f"{name}_full", c, fin, st)
        print("  missing tokens:", [t for t in TOKENS if t not in plain(c)])
if "columns" in scenarios:
    for name, exp in (("two_column", two_col), ("narrow_unequal_seed2", narrow)):
        pdf = f"{FIX}/{name}.pdf"
        c, fin, st = run(render(pdf, side=side))
        say(f"{name}_full", c, fin, st)
        f, o, p = score_lines(plain(c), exp)
        print(f"  FULL  {name}: lines {f}/{len(exp)} ({100*f/len(exp):.1f}%) in-order {o}/{p}")
        crops = subprocess.run([CROPS, pdf, "1"], capture_output=True, text=True, check=True).stdout.split()
        texts, fins, tot = [], [], 0.0
        for i, crop in enumerate(crops):
            c, fin, st = run(render(pdf, crop=crop, side=side))
            say(f"{name}_crop{i}", c, fin, st)
            texts.append(plain(c)); fins.append(fin); tot += st["wall_s"]
        f, o, p = score_lines("\n".join(texts), exp)
        print(f"  CROPS {name}: {len(crops)} crops {fins} lines {f}/{len(exp)} ({100*f/len(exp):.1f}%) in-order {o}/{p} wall {tot:.1f}s")
if "transcripts" in scenarios:
    c, fin, st = run(render(f"{FIX}/transcript_full.pdf", side=side))
    say("transcript_full", c, fin, st)
    lines = re.compile(r"(?m)^\s*(\d+)\s+([QA])\.\s+Line (\d+) of the testimony")
    t = plain(c)
    ok = sum(any(m[1] == str(n) and m[2] == ("Q" if n % 2 else "A") and m[3] == str(n) for m in lines.finditer(t)) for n in range(1, 26))
    print(f"  transcript_full: {ok}/25 exact (line number + Q/A + text)")
    c, fin, st = run(render(f"{FIX}/four_up.pdf", side=side))
    say("four_up", c, fin, st)
    t = plain(c)
    ok = sum(bool(re.search(rf"\b{n}\s+Q\.\s+Question {k}\.{n} about the meeting", t)) for k in range(1, 5) for n in range(1, 26))
    print(f"  four_up: {ok}/100 exact")
if "rot90" in scenarios:
    c, fin, st = run(render(f"{FIX}/legal_scan_rot90.png", side=side))
    say("rot90", c, fin, st)
    print("  head:", c[:200].replace("\n", " | "))
if "dpi" in scenarios:
    for dpi in (100, 150, 200):
        c, fin, st = run(render(f"{FIX}/legal_scan.pdf", dpi=dpi))
        say(f"legal_scan_dpi{dpi}", c, fin, st)
        print(f"  dpi {dpi}: missing", [t for t in TOKENS if t not in plain(c)])
