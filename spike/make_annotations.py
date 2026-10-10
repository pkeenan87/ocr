"""Writes spike/fixtures/annotations.pdf: page 1 has a filled AcroForm text field (`$9,999.00`)
and a FreeText annotation (`AMENDED`), content that is drawn only as annotation appearances, so
no renderer-fed OCR engine sees it; page 2 is plain text with only a Link annotation (not flagged).

usage: spike/.venv/bin/python -I spike/make_annotations.py
"""
import os

from reportlab.lib.pagesizes import letter
from reportlab.pdfbase.pdfdoc import PDFString
from reportlab.pdfgen import canvas

out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures", "annotations.pdf")
W, H = letter
c = canvas.Canvas(out, pagesize=letter)
c.setFont("Times-Roman", 12)
c.drawString(72, H - 72, "Invoice summary for Case No. 3:26-cv-04417.")
c.drawString(72, H - 100, "Total amount due on the Supply Agreement dated March 3, 2024:")
c.acroForm.textfield(
    name="total",
    value="$9,999.00",
    x=72,
    y=H - 150,
    width=160,
    height=24,
    borderStyle="solid",
    forceBorder=True,
    fontName="Times-Roman",
    fontSize=12,
)
c.freeTextAnnotation(
    "AMENDED", PDFString("/Helv 12 Tf 0 g"), Rect=(300, H - 150, 420, H - 120), addtopage=1
)
c.showPage()
c.setFont("Times-Roman", 12)
c.drawString(72, H - 72, "Second page: plain text with a link only.")
c.drawString(72, H - 100, "Payment is due within thirty (30) days of notice.")
c.linkURL("https://example.com", (72, H - 104, 300, H - 90), relative=0)
c.showPage()
c.save()
