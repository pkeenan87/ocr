"""Held-out layouts the Phase 2 column thresholds were NOT tuned on. Writes heldout/*.pdf."""
import sys
from reportlab.lib.pagesizes import letter
from reportlab.pdfgen import canvas
out = sys.argv[1]; W, H = letter

def page(name, draw):
    c = canvas.Canvas(f"{out}/{name}.pdf", pagesize=letter); draw(c); c.save()

def narrow_unequal(c):  # 62% / 34% columns, 12pt gutter
    c.setFont("Times-Roman", 10)
    for i in range(1, 21):
        y = H - 60 - i * 30
        c.drawString(50, y, f"Main clause {i} sets out the indemnity terms."); c.drawString(50, y - 12, "Each party shall comply in full.")
        c.drawString(50 + 0.62 * (W - 100) + 12, y, f"Note {i} defines a term."); c.drawString(50 + 0.62 * (W - 100) + 12, y - 12, "See schedule B.")

def lower_half_columns(c):
    c.setFont("Times-Roman", 11)
    for i in range(1, 11):
        c.drawString(60, H - 60 - i * 22, f"Recital {i}: the parties wish to enter into this agreement on these terms.")
    for i in range(1, 11):
        y = H / 2 - 20 - i * 30
        c.drawString(60, y, f"Seller warranty {i} is given"); c.drawString(60, y - 12, "as of the closing date.")
        c.drawString(W / 2 + 20, y, f"Buyer covenant {i} survives"); c.drawString(W / 2 + 20, y - 12, "for two years after closing.")

def block_quote(c):
    c.setFont("Times-Roman", 12); y = H - 72
    for i in range(1, 8):
        c.drawString(72, y, f"Argument paragraph {i} explains why the motion should be granted in full."); y -= 22
    c.setFont("Times-Italic", 11)
    for i in range(1, 6):
        c.drawString(130, y, f"Quoted holding line {i} from the appellate decision."); y -= 18
    c.setFont("Times-Roman", 12)
    for i in range(8, 14):
        c.drawString(72, y, f"Argument paragraph {i} explains why the motion should be granted in full."); y -= 22

def signature_block(c):
    c.setFont("Times-Roman", 12); y = H - 72
    for i in range(1, 15):
        c.drawString(72, y, f"Body paragraph {i} states the agreed terms of settlement."); y -= 22
    y -= 20
    for t in ("Dated: October 7, 2026", "Respectfully submitted,", "/s/ Jane Q. Counsel", "Jane Q. Counsel", "Attorney for Plaintiff"):
        c.drawRightString(W - 72, y, t); y -= 18

def four_up(c):  # deposition minuscript: 2x2 mini pages with 25 numbered lines each
    c.setFont("Courier", 6.5)
    cw, ch = (W - 60) / 2, (H - 60) / 2
    for k, (ox, oy) in enumerate([(30, H / 2), (30 + cw, H / 2), (30, 30), (30 + cw, 30)], start=1):
        c.rect(ox + 4, oy + 4, cw - 8, ch - 8)
        c.drawString(ox + cw / 2 - 20, oy + ch - 20, f"Page {k + 40}")
        for ln in range(1, 26):
            c.drawString(ox + 12, oy + ch - 30 - ln * 11, f"{ln:>2}  Q. Question {k}.{ln} about the meeting?")

for n, f in [("narrow_unequal", narrow_unequal), ("lower_half_columns", lower_half_columns),
             ("block_quote", block_quote), ("signature_block", signature_block), ("four_up", four_up)]:
    page(n, f)
print("ok")
