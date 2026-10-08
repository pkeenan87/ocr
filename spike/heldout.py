"""Seeded, randomized held-out layouts (one page per family) plus degraded scan copies.

usage: heldout.py --seed N --out DIR
Writes DIR/<name>.pdf, DIR/<name>_scan.pdf and DIR/<name>.expected.json (sentence prefixes in
reading order; the scan shares it). Tune on seeds 0-9 only; seeds 10-39 are for acceptance.
"""
import argparse, io, json, os, random
from reportlab.lib.pagesizes import letter
from reportlab.lib.utils import ImageReader
from reportlab.pdfbase.pdfmetrics import stringWidth
from reportlab.pdfgen import canvas
import pypdfium2 as pdfium
from PIL import Image, ImageFilter

W, H = letter
WORDS = ("the parties agree that each obligation under this section survives termination and "
         "shall be performed in good faith without waiver of any remedy available at law or in "
         "equity subject to the notice provisions set out below").split()


def filler(rng, n):
    return " ".join(rng.choice(WORDS) for _ in range(n))


def wrap(text, font, size, width):
    lines, cur = [], ""
    for w in text.split():
        trial = f"{cur} {w}".strip()
        if cur and stringWidth(trial, font, size) > width:
            lines.append(cur)
            cur = w
        else:
            cur = trial
    return lines + [cur]


def column(c, x, y, width, font, size, pitch, bottom, paras):
    """Draws wrapped paragraphs top-down; returns how many fit."""
    c.setFont(font, size)
    done = 0
    for p in paras:
        lines = wrap(p, font, size, width)
        if y - (len(lines) - 1) * pitch < bottom:
            break
        for ln in lines:
            c.drawString(x, y, ln)
            y -= pitch
        y -= pitch * 0.6
        done += 1
    return done


def narrow_unequal(c, rng):
    ratio, gutter, size = rng.uniform(0.55, 0.7), rng.uniform(10, 24), rng.uniform(9, 11)
    left_w = ratio * (W - 100 - gutter)
    right_w = (1 - ratio) * (W - 100 - gutter)
    pitch, top, bottom = size * 1.2, H - 60, 60
    n = rng.randint(14, 22)
    left = [f"Main clause {i} sets out the {filler(rng, rng.randint(14, 30))}." for i in range(1, n + 1)]
    right = [f"Note {i} defines a {filler(rng, rng.randint(8, 16))}." for i in range(1, n + 1)]
    kl = column(c, 50, top, left_w, "Times-Roman", size, pitch, bottom, left)
    kr = column(c, 50 + left_w + gutter, top, right_w, "Times-Roman", size, pitch, bottom, right)
    return [f"Main clause {i} sets out" for i in range(1, kl + 1)] + [f"Note {i} defines a" for i in range(1, kr + 1)]


def lower_half_columns(c, rng):
    start = rng.uniform(0.35, 0.60) * H  # distance of the columns from the page top
    size, gutter = rng.uniform(10, 11.5), rng.uniform(18, 40)
    c.setFont("Times-Roman", size)
    y, k = H - 60, 0
    exp = []
    while y > H - start + 4 * size:
        k += 1
        c.drawString(60, y, f"Recital {k}: the parties wish to enter into this agreement on these terms.")
        exp.append(f"Recital {k}:")
        y -= size * 1.6
    colw = (W - 120 - gutter) / 2
    top = y - size * 2.5
    n = rng.randint(14, 22)
    left = [f"Seller warranty {i} is given {filler(rng, rng.randint(8, 18))}." for i in range(1, n + 1)]
    right = [f"Buyer covenant {i} survives {filler(rng, rng.randint(8, 18))}." for i in range(1, n + 1)]
    kl = column(c, 60, top, colw, "Times-Roman", size, size * 1.25, 60, left)
    kr = column(c, 60 + colw + gutter, top, colw, "Times-Roman", size, size * 1.25, 60, right)
    return exp + [f"Seller warranty {i} is" for i in range(1, kl + 1)] + [f"Buyer covenant {i} survives" for i in range(1, kr + 1)]


def two_col_title_footer(c, rng):
    size, gutter = rng.uniform(10, 11.5), rng.uniform(20, 36)
    title = rng.choice(["ASSET PURCHASE AGREEMENT", "MASTER SERVICES AGREEMENT", "LICENSE AND SUPPLY CONTRACT"])
    c.setFont("Times-Bold", 14)
    c.drawCentredString(W / 2, H - 60, title)
    colw = (W - 108 - gutter) / 2
    n = rng.randint(14, 22)
    left = [f"Left column sentence {i} concerns {filler(rng, rng.randint(6, 14))}." for i in range(1, n + 1)]
    right = [f"Right column sentence {i} concerns {filler(rng, rng.randint(6, 14))}." for i in range(1, n + 1)]
    top = H - 110
    kl = column(c, 54, top, colw, "Times-Roman", size, size * 1.25, 80, left)
    kr = column(c, 54 + colw + gutter, top, colw, "Times-Roman", size, size * 1.25, 80, right)
    page = rng.randint(2, 30)
    foot = f"CONFIDENTIAL - Page {page} of 31"
    bates = f"ACME{rng.randint(100000, 999999)}"
    c.setFont("Times-Roman", 9)
    c.drawCentredString(W / 2, 36, foot)
    c.drawRightString(W - 54, 36, bates)
    return [title] + [f"Left column sentence {i} concerns" for i in range(1, kl + 1)] + \
        [f"Right column sentence {i} concerns" for i in range(1, kr + 1)] + [foot, bates]


def block_quote(c, rng):
    indent, size = rng.uniform(40, 90), 12
    c.setFont("Times-Roman", size)
    y = H - 72
    a, q, b = rng.randint(5, 9), rng.randint(4, 7), rng.randint(5, 9)
    exp = []
    for i in range(1, a + 1):
        c.drawString(72, y, f"Argument paragraph {i} explains why the motion should be granted in full.")
        exp.append(f"Argument paragraph {i} explains"); y -= 22
    c.setFont("Times-Italic", 11)
    for i in range(1, q + 1):
        c.drawString(72 + indent, y, f"Quoted holding line {i} from the appellate decision.")
        exp.append(f"Quoted holding line {i} from"); y -= 18
    c.setFont("Times-Roman", size)
    for i in range(a + 1, a + b + 1):
        c.drawString(72, y, f"Argument paragraph {i} explains why the motion should be granted in full.")
        exp.append(f"Argument paragraph {i} explains"); y -= 22
    return exp


def signature_block(c, rng):
    c.setFont("Times-Roman", 12)
    y = H - 72
    n = rng.randint(10, 16)
    exp = []
    for i in range(1, n + 1):
        c.drawString(72, y, f"Body paragraph {i} states the agreed terms of settlement.")
        exp.append(f"Body paragraph {i} states"); y -= 22
    y -= rng.uniform(10, 30)
    lines = ("Dated: October 7, 2026", "Respectfully submitted,", "/s/ Jane Q. Counsel", "Jane Q. Counsel", "Attorney for Plaintiff")
    for t in lines:
        c.drawRightString(W - 72, y, t); y -= 18
    return exp + [t for t in lines if not t.startswith("/s/")]


FAMILIES = [narrow_unequal, lower_half_columns, two_col_title_footer, block_quote, signature_block]


def degrade(pdf_path, out_path, rng):
    """Same degradation as make_fixture.py: skew, speckle, blur, JPEG q=55, image-only PDF."""
    scan = pdfium.PdfDocument(pdf_path)[0].render(scale=300 / 72).to_pil().convert("L")
    scan = scan.rotate(0.5 * rng.choice((-1, 1)), resample=Image.BICUBIC, expand=False, fillcolor=255)
    px = scan.load()
    w, h = scan.size
    for _ in range(w * h // 150):
        x, y = rng.randrange(w), rng.randrange(h)
        px[x, y] = rng.choice((0, 90, 180))
    scan = scan.filter(ImageFilter.GaussianBlur(0.6))
    buf = io.BytesIO()
    scan.save(buf, "JPEG", quality=55)
    buf.seek(0)
    c = canvas.Canvas(out_path, pagesize=letter)
    c.drawImage(ImageReader(buf), 0, 0, W, H)
    c.save()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", type=int, required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    for fam in FAMILIES:
        name = fam.__name__
        rng = random.Random(f"{args.seed}:{name}")
        pdf = f"{args.out}/{name}.pdf"
        c = canvas.Canvas(pdf, pagesize=letter)
        expected = fam(c, rng)
        c.save()
        with open(f"{args.out}/{name}.expected.json", "w") as f:
            json.dump(expected, f, indent=1)
        degrade(pdf, f"{args.out}/{name}_scan.pdf", rng)
    print("ok")


if __name__ == "__main__":
    main()
