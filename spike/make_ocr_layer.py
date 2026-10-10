"""Writes spike/fixtures/ocr_layer.pdf: a scanned page (an image of text) with an invisible
OCR text layer over it (render mode 3) whose words are misread (`qction`, `Cqurt`), as old OCR
tools leave them. Embedded-text correction must not take those characters.

Also writes spike/fixtures/ocr_layer_alpha.pdf, the same with the layer drawn in render mode 0
but fully transparent (fill alpha 0), as iOS scanner apps write it.

Also writes spike/fixtures/ocr_layer_form.pdf (the invisible layer drawn inside a Form XObject, with
its own resources) and spike/fixtures/ocr_layer_mixed.pdf (the hidden layer plus a visible header of
more text-showing operators than the layer has but far fewer characters: counting operators would
call the page real text, counting characters does not).

usage: spike/.venv/bin/python -I spike/make_ocr_layer.py
"""
import io
import os

from PIL import Image, ImageDraw, ImageFont
from reportlab.lib.pagesizes import letter
from reportlab.lib.utils import ImageReader
from reportlab.pdfgen import canvas

LINES = [
    "Plaintiff complains of defendant and for cause of action",
    "alleges that the Court has jurisdiction over this action",
    "and that venue is proper in this district under the statute.",
]
OCR = [l.replace("action", "qction").replace("Court", "Cqurt") for l in LINES]

dpi = 200
w, h = int(8.5 * dpi), int(11 * dpi)
img = Image.new("L", (w, h), 255)
draw = ImageDraw.Draw(img)
font = ImageFont.truetype("/System/Library/Fonts/Supplemental/Times New Roman.ttf", 40)
for i, line in enumerate(LINES):
    draw.text((200, 300 + i * 80), line, fill=0, font=font)
buf = io.BytesIO()
img.save(buf, format="PNG")

for name, alpha in [("ocr_layer.pdf", False), ("ocr_layer_alpha.pdf", True)]:
    out = os.path.join(os.path.dirname(__file__), "fixtures", name)
    c = canvas.Canvas(out, pagesize=letter)
    c.drawImage(ImageReader(io.BytesIO(buf.getvalue())), 0, 0, width=letter[0], height=letter[1])
    if alpha:
        c.setFillAlpha(0)
    t = c.beginText()
    t.setTextRenderMode(0 if alpha else 3)
    t.setFont("Times-Roman", 14.4)
    for i, line in enumerate(OCR):
        t.setTextOrigin(72, letter[1] - (300 + i * 80 + 30) * 72 / dpi)
        t.textLine(line)
    c.drawText(t)
    c.showPage()
    c.save()
    print(out)


def layer_text(c, lines, origin_y):
    t = c.beginText()
    t.setTextRenderMode(3)
    t.setFont("Times-Roman", 14.4)
    for i, line in enumerate(lines):
        t.setTextOrigin(72, origin_y(i))
        t.textLine(line)
    return t


def origin_y(i):
    return letter[1] - (300 + i * 80 + 30) * 72 / dpi


def scan_page(c):
    c.drawImage(ImageReader(io.BytesIO(buf.getvalue())), 0, 0, width=letter[0], height=letter[1])


here = os.path.join(os.path.dirname(__file__), "fixtures")

# The layer inside a Form XObject.
out = os.path.join(here, "ocr_layer_form.pdf")
c = canvas.Canvas(out, pagesize=letter)
scan_page(c)
c.beginForm("ocrlayer")
c.drawText(layer_text(c, OCR, origin_y))
c.endForm()
c.doForm("ocrlayer")
c.showPage()
c.save()
print(out)

# The layer plus a visible header: 8 short visible Tj against the layer's 3 long hidden ones.
out = os.path.join(here, "ocr_layer_mixed.pdf")
c = canvas.Canvas(out, pagesize=letter)
scan_page(c)
c.drawText(layer_text(c, OCR, origin_y))
c.setFont("Helvetica", 9)
for i, word in enumerate(["Page", "1", "of", "1", "Case", "File", "No.", "Copy"]):
    c.drawString(72, letter[1] - 30 - i * 10, word)
c.showPage()
c.save()
print(out)
