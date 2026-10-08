"""Build a synthetic one-page legal filing with known ground truth."""
import sys
from reportlab.lib.pagesizes import letter
from reportlab.pdfgen import canvas
import pypdfium2 as pdfium

out = sys.argv[1]
W, H = letter
c = canvas.Canvas(f"{out}/legal.pdf", pagesize=letter)
# Pleading-paper line numbers
c.setFont("Times-Roman", 10)
for i in range(1, 29):
    c.drawRightString(60, H - 72 - (i - 1) * 24, str(i))
c.line(66, 40, 66, H - 40)
x = 80
y = H - 72
def line(t, font="Times-Roman", size=12, dx=0):
    global y
    c.setFont(font, size); c.drawString(x + dx, y, t); y -= 24
c.setFont("Times-Bold", 13)
c.drawCentredString(W / 2 + 30, y, "UNITED STATES DISTRICT COURT"); y -= 24
c.drawCentredString(W / 2 + 30, y, "NORTHERN DISTRICT OF CALIFORNIA"); y -= 36
line("SMITH HOLDINGS, LLC, Plaintiff, v. JONES MANUFACTURING, INC., Defendant.")
line("Case No. 3:26-cv-04417", "Times-Bold")
line("1. Plaintiff brings this action for breach of the Supply Agreement dated")
line("March 3, 2024, under which Defendant agreed to deliver 12,500 units.")
line("2. Defendant failed to perform as follows:")
line("a. Late delivery of the first shipment;", dx=20)
line("b. Nonconforming goods in the second shipment; and", dx=20)
line("c. Failure to cure within thirty (30) days of notice.", dx=20)
y -= 6
rows = [("Invoice", "Date", "Amount"), ("INV-1001", "04/15/2024", "$48,250.00"),
        ("INV-1002", "06/01/2024", "$51,900.00"), ("INV-1003", "08/20/2024", "$22,375.50")]
tx, cw, rh = x + 20, 130, 22
for r, row in enumerate(rows):
    for col, val in enumerate(row):
        c.rect(tx + col * cw, y - 6, cw, rh)
        c.setFont("Times-Bold" if r == 0 else "Times-Roman", 11)
        c.drawString(tx + col * cw + 6, y, val)
    y -= rh
y -= 18
line("3. The total amount due and owing is $122,525.50, plus interest.")
c.setFont("Times-Roman", 9)
c.drawCentredString(W / 2, 30, "COMPLAINT - Page 1 of 14")
c.drawRightString(W - 40, 30, "SMITH000123")
c.save()

pdf = pdfium.PdfDocument(f"{out}/legal.pdf")
page = pdf[0]
page.render(scale=300 / 72).to_pil().save(f"{out}/legal_300dpi.png")
img = page.render(scale=1288 / (H)).to_pil()
img.save(f"{out}/legal_1288.png")
print(img.size)

# --- Phase 2 fixtures -------------------------------------------------------
import io, random
from PIL import Image, ImageFilter
from reportlab.lib.utils import ImageReader

# Degraded scan: image-only PDF, 300 DPI, 0.5° skew, noise, blur, JPEG q=55.
random.seed(7)
scan = pdfium.PdfDocument(f"{out}/legal.pdf")[0].render(scale=300 / 72).to_pil().convert("L")
scan = scan.rotate(0.5, resample=Image.BICUBIC, expand=False, fillcolor=255)
px = scan.load()
w, h = scan.size
for _ in range(w * h // 150):
    x, y = random.randrange(w), random.randrange(h)
    px[x, y] = random.choice((0, 90, 180))
scan = scan.filter(ImageFilter.GaussianBlur(0.6))
buf = io.BytesIO(); scan.save(buf, "JPEG", quality=55); buf.seek(0)
c = canvas.Canvas(f"{out}/legal_scan.pdf", pagesize=letter)
c.drawImage(ImageReader(buf), 0, 0, W, H)
c.save()

# Two-column page: each column is a numbered sequence so order is checkable.
c = canvas.Canvas(f"{out}/two_column.pdf", pagesize=letter)
c.setFont("Times-Bold", 14)
c.drawCentredString(W / 2, H - 60, "ASSET PURCHASE AGREEMENT")
c.setFont("Times-Roman", 11)
left = [f"Left column sentence {i} concerns the transfer of assets." for i in range(1, 16)]
right = [f"Right column sentence {i} concerns the purchase price." for i in range(1, 16)]
for i, (l, r) in enumerate(zip(left, right)):
    yy = H - 100 - i * 30
    for xx, sent in ((54, l), (W / 2 + 18, r)):
        head, tail = sent.split(" concerns ")
        c.drawString(xx, yy, head + " concerns"); c.drawString(xx, yy - 13, tail)
c.save()

# Multi-page TIFF (2 frames) and a 3-page PDF combining the fixtures.
frames = [pdfium.PdfDocument(f"{out}/{n}")[0].render(scale=200 / 72).to_pil().convert("L") for n in ("legal.pdf", "two_column.pdf")]
frames[0].save(f"{out}/multipage.tif", save_all=True, append_images=frames[1:], compression="tiff_lzw")
merged = pdfium.PdfDocument.new()
for n in ("legal.pdf", "two_column.pdf", "legal_scan.pdf"):
    merged.import_pages(pdfium.PdfDocument(f"{out}/{n}"))
merged.save(f"{out}/three_pages.pdf")
print("phase 2 fixtures written")
