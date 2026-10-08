#!/usr/bin/env python3
"""Phase 6 evaluation fixture generator with exact ground truth.

usage: generate.py --seed N --out DIR [--families pleading,contract,...]

Writes <family>.pdf and <family>.gt.json per family. Deterministic for a given seed (reportlab runs
in invariant mode, all randomness comes from seeded generators).

Ground truth: {"family", "seed", "pages": [{"text", "tokens", "bates", "kind", "furniture"}], "notes"}.
`text` is the reading-order body of the page; paragraphs are separated by blank lines, table rows
are cells joined by " | ". `furniture` (an extra key) lists running headers, footers, fax headers
and Bates strings that are drawn on the page but are not part of `text`; the scorer ignores them in
the model output so engines are neither rewarded nor punished for keeping them.

Content is built from real-sentence templates and legal vocabulary (never random words), because
VLMs lean on language priors and word salad made them loop in an earlier spike.

NO HANDWRITING FAMILY: trustworthy ground truth for handwriting cannot be generated synthetically
(a handwriting font is not handwriting), so the PLAN.md handwritten-annotation page is evaluated by
hand, outside this harness.
"""
import argparse, io, json, os, random, re, sys, time
from datetime import date, timedelta

import numpy as np
import pypdfium2 as pdfium
from PIL import Image, ImageFilter
from reportlab.lib.pagesizes import letter
from reportlab.lib.utils import ImageReader
from reportlab.pdfbase.pdfmetrics import stringWidth
from reportlab.pdfgen import canvas

W, H = letter

# ------------------------------------------------------------------ vocabulary

WORDS = ["Harbor", "Summit", "Pioneer", "Meridian", "Atlas", "Cedar", "Northgate", "Bluewater",
         "Ironwood", "Lakeside", "Redwood", "Crescent", "Granite", "Westfield", "Sterling", "Evergreen",
         "Falcon", "Brookline", "Cobalt", "Highland", "Keystone", "Silverton", "Oakmont", "Riverbend"]
SUFFIXES = [("Holdings, LLC", "limited liability company"), ("Manufacturing, Inc.", "corporation"),
            ("Industries, Inc.", "corporation"), ("Partners, LP", "limited partnership"),
            ("Logistics, LLC", "limited liability company"), ("Technologies, Inc.", "corporation"),
            ("Supply Co.", "corporation")]
FIRST = ["James", "Maria", "Robert", "Linda", "David", "Susan", "Michael", "Karen", "Thomas", "Patricia",
         "Daniel", "Angela", "Richard", "Nancy", "Kevin", "Laura"]
LAST = ["Anderson", "Brooks", "Castillo", "Donovan", "Ellison", "Foster", "Grant", "Hughes", "Ingram",
        "Jenkins", "Keller", "Lawson", "Mitchell", "Nguyen", "Ortega", "Preston", "Quinn", "Reyes"]
CITIES = [("Houston", "Texas", "TX"), ("Chicago", "Illinois", "IL"), ("Denver", "Colorado", "CO"),
          ("Atlanta", "Georgia", "GA"), ("Phoenix", "Arizona", "AZ"), ("Columbus", "Ohio", "OH"),
          ("Portland", "Oregon", "OR"), ("Tampa", "Florida", "FL"), ("Boston", "Massachusetts", "MA"),
          ("Charlotte", "North Carolina", "NC")]
GOODS = ["industrial pumps", "steel fasteners", "refrigeration units", "circuit assemblies",
         "packaging materials", "hydraulic valves", "conveyor components", "stainless steel fittings"]
STREETS = ["Main", "Commerce", "Industrial", "Maple", "Harbor", "Lincoln", "Market", "Elm"]
STREET_TYPES = ["Street", "Avenue", "Boulevard", "Parkway"]
MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September",
          "October", "November", "December"]
DAYS_W = ["ten (10)", "fifteen (15)", "thirty (30)", "forty-five (45)", "sixty (60)", "ninety (90)"]
TITLES = ["Chief Executive Officer", "Vice President of Operations", "General Counsel", "Controller",
          "Director of Procurement"]


def rdate(rng, y0=2021, y1=2025):
    return date(y0, 1, 1) + timedelta(days=rng.randrange((date(y1, 12, 28) - date(y0, 1, 1)).days))


def longdate(d):
    return f"{MONTHS[d.month - 1]} {d.day}, {d.year}"


def money(v):
    return f"${v:,.2f}"


class Ctx:
    """Names and values shared by one document. Dynamic fields (dates, amounts) are fresh per use."""

    def __init__(self, rng):
        self.rng = rng
        w = rng.sample(WORDS, 2)
        sa, sb = rng.choice(SUFFIXES), rng.choice(SUFFIXES)
        self.fixed = {
            "A": f"{w[0]} {sa[0]}", "a": w[0], "atype": sa[1],
            "B": f"{w[1]} {sb[0]}", "b": w[1], "btype": sb[1],
            "goods": rng.choice(GOODS),
            "firm": f"{rng.choice(LAST)} & {rng.choice(LAST)} LLP",
        }
        self.city, self.state, self.ab = rng.choice(CITIES)
        self.city2, self.state2, self.ab2 = rng.choice(CITIES)
        self.fixed.update(city=self.city, state=self.state)
        self.pa = self.person()
        self.pb = self.person()

    def person(self):
        return f"{self.rng.choice(FIRST)} {self.rng.choice(LAST)}"

    def street(self):
        r = self.rng
        return f"{r.randint(100, 9800)} {r.choice(STREETS)} {r.choice(STREET_TYPES)}"

    def zip(self):
        return str(self.rng.randint(10000, 99950))

    def amount(self, lo=5000, hi=400000):
        r = self.rng
        v = r.randrange(lo, hi) + (r.choice([0, 0, 0.5, 0.25, 0.75, 0.1]) if r.random() < 0.5 else 0)
        return money(v)

    def get(self, k):
        r = self.rng
        if k in self.fixed:
            return self.fixed[k]
        if k == "date":
            return longdate(rdate(r))
        if k == "money":
            return self.amount()
        if k == "small":
            return self.amount(500, 49000)
        if k == "n":
            return f"{r.randrange(500, 40000):,}"
        if k == "inv":
            return f"INV-{r.randint(1000, 99999)}"
        if k == "days":
            return r.choice(DAYS_W)
        if k == "pct":
            return r.choice(["8", "9", "10", "12", "1.5"])
        if k == "years":
            return r.choice(["two (2)", "three (3)", "five (5)"])
        if k == "sec":
            return f"{r.randint(3, 12)}.{r.randint(1, 6)}"
        if k == "pa":
            return self.pa
        if k == "pb":
            return self.pb
        if k == "title":
            return r.choice(TITLES)
        if k == "month":
            return r.choice(MONTHS)
        if k == "x":
            return str(r.randint(1, 30))
        if k == "sched":
            return str(r.randint(1, 6))
        raise KeyError(k)

    def fill(self, tpl):
        return re.sub(r"\{(\w+)\}", lambda m: self.get(m.group(1)), tpl)


# ------------------------------------------------------------------ pages and layout

class Page:
    def __init__(self, kind="body"):
        self.items, self.parts, self.furniture = [], [], []
        self.bates, self.kind = None, kind

    def text(self, x, y, t, font="Times-Roman", size=12, align="l"):
        self.items.append(("t", x, y, t, font, size, align))

    def line(self, x1, y1, x2, y2, w=0.8):
        self.items.append(("line", x1, y1, x2, y2, w))

    def rect(self, x, y, w, h):
        self.items.append(("rect", x, y, w, h))

    def gt_text(self):
        return "\n\n".join(self.parts)


def render_page(c, p):
    draw = {"l": c.drawString, "c": c.drawCentredString, "r": c.drawRightString}
    for it in p.items:
        if it[0] == "t":
            _, x, y, t, font, size, align = it
            c.setFont(font, size)
            draw[align](x, y, t)
        elif it[0] == "line":
            c.setLineWidth(it[5])
            c.line(*it[1:5])
        else:
            c.setLineWidth(0.6)
            c.rect(*it[1:5], stroke=1, fill=0)
    c.showPage()


def wrap(text, font, size, width, indent=0):
    lines, cur = [], ""
    for w in text.split():
        trial = (cur + " " + w).strip()
        lim = width - (indent if not lines else 0)
        if stringWidth(trial, font, size) <= lim or not cur:
            cur = trial
        else:
            lines.append(cur)
            cur = w
    if cur:
        lines.append(cur)
    return lines


class Flow:
    """Top-down text placement inside a region of one page."""

    def __init__(self, page, x0, x1, top, bottom):
        self.page, self.x0, self.x1, self.top, self.bottom, self.y = page, x0, x1, top, bottom, top

    def used(self):
        return (self.top - self.y) / (self.top - self.bottom)

    def para(self, text, font="Times-Roman", size=11, leading=None, indent=0, gap=6, align="l",
             gt=True, keep=0):
        leading = leading or size * 1.25
        lines = wrap(text, font, size, self.x1 - self.x0, indent)
        if self.y - (len(lines) - 1) * leading - gap - keep < self.bottom:
            return False
        for i, ln in enumerate(lines):
            x = {"l": self.x0 + (indent if i == 0 else 0), "c": (self.x0 + self.x1) / 2, "r": self.x1}[align]
            self.page.text(x, self.y, ln, font, size, align)
            self.y -= leading
        self.y -= gap
        if gt:
            self.page.parts.append(text)
        return True

    def lines(self, ls, font="Times-Roman", size=11, leading=None, gap=6, align="l", keep=0):
        leading = leading or size * 1.25
        if self.y - (len(ls) - 1) * leading - gap - keep < self.bottom:
            return False
        for ln in ls:
            x = {"l": self.x0, "c": (self.x0 + self.x1) / 2, "r": self.x1}[align]
            self.page.text(x, self.y, ln, font, size, align)
            self.y -= leading
        self.y -= gap
        self.page.parts.append("\n".join(ls))
        return True


# ------------------------------------------------------------------ degradation recipes

def rasterize(page, dpi):
    buf = io.BytesIO()
    c = canvas.Canvas(buf, pagesize=letter, invariant=1)
    render_page(c, page)
    c.save()
    return pdfium.PdfDocument(buf.getvalue())[0].render(scale=dpi / 72).to_pil().convert("L")


def scan_recipe(img, seed, skew=0.5, quality=55):
    """spike/make_fixture.py: skew, speckle (1 px in 150 set to 0/90/180), blur 0.6, JPEG q=55."""
    g = np.random.default_rng(seed)
    img = img.rotate(skew, resample=Image.BICUBIC, expand=False, fillcolor=255)
    a = np.array(img)
    n = a.size // 150
    idx = g.integers(0, a.size, n)
    a.reshape(-1)[idx] = g.choice([0, 90, 180], n)
    img = Image.fromarray(a).filter(ImageFilter.GaussianBlur(0.6))
    buf = io.BytesIO()
    img.save(buf, "JPEG", quality=quality)
    buf.seek(0)
    return buf


def fax_recipe(img, seed, skew):
    """Skew, hard threshold (no gray), salt-and-pepper speckle, JPEG q=40."""
    g = np.random.default_rng(seed)
    img = img.rotate(skew, resample=Image.BICUBIC, expand=False, fillcolor=255)
    a = np.where(np.array(img) < 160, 0, 255).astype(np.uint8)
    n = int(a.size * 0.0025)
    idx = g.integers(0, a.size, n)
    a.reshape(-1)[idx] = g.choice([0, 255], n)
    buf = io.BytesIO()
    Image.fromarray(a).save(buf, "JPEG", quality=40)
    buf.seek(0)
    return buf


def draw_image_page(c, jpeg):
    c.drawImage(ImageReader(jpeg), 0, 0, W, H)
    c.showPage()


# ------------------------------------------------------------------ ground truth helpers

TOKEN_RES = [r"\d:\d{2}-cv-\d{5}", r"\$\d[\d,]*(?:\.\d\d)?",
             r"(?:" + "|".join(MONTHS) + r") \d{1,2}, \d{4}", r"\d{2}/\d{2}/\d{4}",
             r"INV-\d+", r"[A-Z]{4,6}\d{6,7}"]


def tokens_of(text, extra=()):
    found = []
    for rx in TOKEN_RES:
        found += [(m.start(), m.group()) for m in re.finditer(rx, text)]
    out = []
    for _, t in sorted(found):
        if t not in out:
            out.append(t)
    for t in extra:
        if t and t not in out:
            out.append(t)
    return out


def gt_page(p, extra_tokens=()):
    text = p.gt_text()
    return {"text": text, "tokens": tokens_of(text, list(extra_tokens) + ([p.bates] if p.bates else [])),
            "bates": p.bates, "kind": p.kind, "furniture": p.furniture}


# ------------------------------------------------------------------ shared prose

def legal_paras(ctx):
    """Letter and email body paragraphs."""
    return [
        "I am writing on behalf of {A} regarding the outstanding balance of {money} on Invoice No. {inv}, which was due on {date}.",
        "Our records indicate that the {goods} shipped on {date} were received at your {city} facility.",
        "Please remit payment in full no later than {date}. If we do not receive payment by that date, we will refer this matter to our counsel without further notice.",
        "Enclosed please find a copy of the Purchase Order dated {date} and the corresponding packing slip.",
        "As discussed during our call on {date}, {B} will return the nonconforming units within {days} days.",
        "The Supply Agreement between our companies requires written notice of any defect within {days} days of delivery, and we did not receive such notice.",
        "We have credited your account in the amount of {small} for the damaged {goods} reported on {date}.",
        "Delivery of the remaining {n} units is scheduled for {date}, subject to confirmation of your revised purchase order.",
        "We appreciate your prompt attention to this matter and look forward to resolving it amicably.",
        "Please contact me directly if you would like to discuss a revised payment schedule.",
    ]


def pick_paras(ctx, k):
    pool = legal_paras(ctx)
    return [ctx.fill(t) for t in ctx.rng.sample(pool, k)]


def draw_letter(ctx, fl, paras, sig=True, size=11):
    r = ctx.rng
    fl.para(ctx.get("A").upper(), "Times-Bold", 14, 17, align="c", gap=0)
    fl.para(f"{ctx.street()}, {ctx.city}, {ctx.ab} {ctx.zip()}", "Times-Roman", 10, 12, align="c", gap=16)
    fl.para(longdate(rdate(r, 2022, 2025)), gap=10)
    fl.lines([ctx.pb, r.choice(TITLES), ctx.get("B"), ctx.street(), f"{ctx.city2}, {ctx.ab2} {ctx.zip()}"], gap=10)
    fl.para(f"Re: Invoice No. {r.randint(1000, 99999)} and Purchase Order dated {longdate(rdate(r))}", "Times-Bold", 11, gap=10)
    fl.para(f"Dear {ctx.pb}:", gap=8)
    for t in paras:
        if not fl.para(t, size=size, leading=size * 1.3, gap=size * 0.75):
            break
    if sig:
        fl.para("Sincerely,", gap=18)
        fl.lines([ctx.pa, r.choice(TITLES)], gap=0)


# ------------------------------------------------------------------ contract text

ORDER = [1, 2, 3, 4, 5, 6, 7, 8, "disp", "emp", "tax", "misc"]


def article_sections(ctx, no):
    """Return (title, [section text]) for article `no` (1-9 order is fixed so cross references hold)."""
    S, B = "Seller", "Buyer"
    r = ctx.rng
    extras = ["The parties acknowledge that this provision is a material inducement to the other party.",
              "Nothing in this Section shall limit any other right or remedy available to the parties under this Agreement.",
              "Each party shall act in good faith in carrying out its obligations under this Section.",
              "Any notice required by this Section shall be given in writing."]
    defs = ["“Assets” means all of the equipment, inventory, customer contracts and books and records of Seller used in connection with the Business.",
            "“Business” means the manufacture and sale of {goods} conducted by Seller at the Facility in {city}, {state}.",
            "“Closing Date” means {date}, or such other date as the parties may agree in writing.",
            "“Purchase Price” has the meaning set forth in Section 3.1.",
            "“Material Adverse Effect” means any event that, individually or in the aggregate, reduces the value of the Assets by more than {money}.",
            "“Liabilities” means all debts, obligations and liabilities of any kind, whether known or unknown.",
            "“Escrow Agent” means {firm}, acting in its capacity as escrow agent under this Agreement."]
    arts = {
        1: ("DEFINITIONS", [("Defined Terms", [d]) for d in r.sample(defs, r.randint(5, 7))]),
        2: ("PURCHASE AND SALE", [
            ("Sale of Assets", ["Subject to the terms and conditions of this Agreement, Seller shall sell, transfer and deliver to Buyer, and Buyer shall purchase from Seller, all of the Assets at the Closing."]),
            ("Excluded Assets", ["The Assets do not include cash, bank accounts or any claims for refund of taxes paid by Seller prior to the Closing Date."]),
            ("Assumed Liabilities", ["Buyer shall assume only those Liabilities of Seller arising under the customer contracts listed on Schedule {sched} after the Closing Date."])]),
        3: ("PAYMENT", [
            ("Purchase Price", ["The aggregate purchase price for the Assets shall be {money} (the “Purchase Price”), payable by wire transfer of immediately available funds."]),
            ("Deposit", ["Within {days} days after the date of this Agreement, Buyer shall deposit {small} with the Escrow Agent as a deposit toward the Purchase Price."]),
            ("Payment Schedule", ["The balance of the Purchase Price shall be paid in four equal quarterly installments of {money}, the first of which is due on {date}."]),
            ("Late Payments", ["Any amount not paid when due shall bear interest at the rate of {pct} percent per annum from the due date until paid in full."])]),
        4: ("CLOSING", [
            ("Time and Place", ["The closing of the transactions contemplated by this Agreement (the “Closing”) shall take place at the offices of {firm} in {city} on the Closing Date."]),
            ("Seller Deliverables", ["At the Closing, Seller shall deliver to Buyer a bill of sale, an assignment of the customer contracts and a certificate of good standing issued by the Secretary of State of {state}."]),
            ("Buyer Deliverables", ["At the Closing, Buyer shall deliver to Seller the balance of the Purchase Price and a certificate executed by an officer of Buyer confirming the accuracy of its representations."])]),
        5: ("REPRESENTATIONS AND WARRANTIES OF SELLER", [
            ("Organization", ["Seller is duly organized, validly existing and in good standing under the laws of the State of {state}."]),
            ("Title to Assets", ["Seller has good and marketable title to the Assets, free and clear of all liens, other than liens for taxes not yet due."]),
            ("Litigation", ["There is no action pending or, to the knowledge of Seller, threatened against Seller that would reasonably be expected to have a Material Adverse Effect."]),
            ("Inventory", ["All inventory of {goods} included in the Assets is of a quality usable and saleable in the ordinary course of business."])]),
        6: ("COVENANTS", [
            ("Conduct of Business", ["Between the date of this Agreement and the Closing Date, Seller shall operate the Business in the ordinary course and shall not sell any Assets valued at more than {small}."]),
            ("Access", ["Seller shall give Buyer and its representatives reasonable access to the Facility and the books and records of the Business during normal business hours."]),
            ("Confidentiality", ["Each party shall keep confidential the terms of this Agreement and all non-public information received from the other party for a period of {years} years."])]),
        7: ("INDEMNIFICATION", [
            ("Indemnification by Seller", ["Seller shall indemnify and hold harmless Buyer from any loss, damage or expense arising out of a breach of any representation or warranty of Seller in this Agreement."]),
            ("Limitation of Liability", ["The aggregate liability of Seller under this Article 7 shall not exceed {money}, and no claim may be made unless written notice is delivered within {days} days after the Closing Date."]),
            ("Claims Procedure", ["A party seeking indemnification shall give the other party prompt written notice of any third-party claim, describing the claim in reasonable detail."])]),
        8: ("TERMINATION", [
            ("Right to Terminate", ["Either party may terminate this Agreement by written notice if the Closing has not occurred on or before {date}."]),
            ("Effect of Termination", ["If this Agreement is terminated, the deposit shall be returned to Buyer within {days} days, and neither party shall have any further liability except under Section 6.3."])]),
        "disp": ("DISPUTE RESOLUTION", [
            ("Negotiation", ["The parties shall first attempt in good faith to resolve any dispute arising out of this Agreement through direct negotiation between senior executives for a period of {days} days."]),
            ("Mediation", ["Any dispute not resolved by negotiation shall be submitted to non-binding mediation in {city}, with the fees of the mediator shared equally by the parties."]),
            ("Arbitration", ["Any dispute not resolved by mediation shall be finally settled by binding arbitration before a single arbitrator, and judgment on the award may be entered in any court of competent jurisdiction."])]),
        "emp": ("EMPLOYEES", [
            ("Offers of Employment", ["Buyer may, in its sole discretion, offer employment to any employee of the Business effective as of the Closing Date."]),
            ("Accrued Compensation", ["Seller shall pay all wages and accrued vacation owed to its employees through the Closing Date, in an aggregate amount not to exceed {small}."]),
            ("Non-Solicitation", ["For a period of {years} years after the Closing Date, Seller shall not solicit for employment any person employed by Buyer in connection with the Business."])]),
        "tax": ("TAXES", [
            ("Transfer Taxes", ["All sales, use and transfer taxes arising from the sale of the Assets shall be borne by Buyer."]),
            ("Allocation", ["The Purchase Price shall be allocated among the Assets in accordance with Section 1060 of the Internal Revenue Code, as set forth on Schedule {sched}."]),
            ("Cooperation", ["Each party shall provide the other with such records as are reasonably necessary to prepare any tax return relating to the Assets."])]),
        "misc": ("MISCELLANEOUS", [
            ("Governing Law", ["This Agreement shall be governed by the laws of the State of {state}, without regard to its conflict of laws principles."]),
            ("Notices", ["All notices under this Agreement shall be in writing and delivered by hand or by overnight courier to the address of the receiving party set forth on the signature page."]),
            ("Entire Agreement", ["This Agreement constitutes the entire agreement of the parties and supersedes all prior negotiations and understandings."]),
            ("Amendment", ["This Agreement may be amended only by a written instrument signed by both parties."])]),
    }
    title, secs = arts[ORDER[no - 1]]
    out = []
    for k, (t, sents) in enumerate(secs, 1):
        body = " ".join(ctx.fill(s) for s in sents)
        if ORDER[no - 1] in (3, 7):
            body += " Any such amount shall be paid without set-off, counterclaim or deduction."
        if no != 1:
            body += " " + " ".join(r.sample(extras, r.randint(1, 3)))
        label = f"{no}.{k} " + ("" if no == 1 else f"{t}. ")
        out.append(label + body)
    return title, out


def build_contract(rng, seed):
    for _attempt in range(40):
        ctx = Ctx(rng)
        n = rng.randint(4, 6)
        pages = []
        state = {}

        def new_page():
            p = Page()
            pages.append(p)
            idx = len(pages) - 1
            if idx == 2:  # the two-column page
                cw = (W - 144 - 20) / 2
                state["cols"] = [(72, 72 + cw), (72 + cw + 20, W - 72)]
                state["col"] = 0
                state["fl"] = Flow(p, *state["cols"][0], H - 90, 70)
            else:
                state["cols"] = None
                state["fl"] = Flow(p, 72, W - 72, H - 90, 70)

        def put(text, font="Times-Roman", size=None, body=False, **kw):
            while True:
                sz_ = size or (10 if state["cols"] else 11)
                if body:
                    kw["leading"] = sz_ * 1.55
                if state["fl"].para(text, font, sz_, **kw):
                    return
                if state["cols"] and state["col"] == 0:
                    state["col"] = 1
                    state["fl"] = Flow(pages[-1], *state["cols"][1], H - 90, 70)
                else:
                    new_page()

        new_page()
        put("ASSET PURCHASE AGREEMENT", "Times-Bold", 16, align="c", gap=14)
        put(ctx.fill("This Asset Purchase Agreement (this “Agreement”) is entered into as of {date}, by and between {A}, "
                     "a {atype} (“Seller”), and {B}, a {btype} (“Buyer”)."), gap=8)
        put("The parties agree as follows:", gap=10)
        for no in range(1, 13):
            if (len(pages) == n and state["fl"].used() > 0.3) or len(pages) > n:
                break
            title, secs = article_sections(ctx, no)
            put(f"ARTICLE {no} \u2014 {title}", "Times-Bold", align="c", gap=6, keep=40)
            for s in secs:
                put(s, body=True, gap=9)
        put("IN WITNESS WHEREOF, the parties have executed this Agreement as of the date first written above.", gap=10, keep=60)
        for who, role, nm in (("SELLER", ctx.get("A"), ctx.pa), ("BUYER", ctx.get("B"), ctx.pb)):
            put(f"{who}: {role}", "Times-Bold", gap=2, keep=30)
            put(f"By: {nm}, {rng.choice(TITLES)}", gap=10)
        if 4 <= len(pages) <= 6:
            break
    header = "CONFIDENTIAL \u2014 Asset Purchase Agreement"
    for i, p in enumerate(pages, 1):
        foot = f"Page {i} of {len(pages)}"
        p.text(W / 2, H - 45, header, "Times-Italic", 9, "c")
        p.text(W / 2, 40, foot, "Times-Roman", 9, "c")
        p.furniture = [header, foot]
    return pages, "Born-digital vector contract. Page 3 is two-column (left column then right column in reading order)."


# ------------------------------------------------------------------ pleading

def build_pleading(rng, seed):
    ctx = Ctx(rng)
    P, D = ctx.get("A"), ctx.get("B")
    atty = ctx.person().upper()
    pfx = ctx.get("a").upper()[:5].ljust(5, "X")
    blocks = [  # (kind, text) kind: c=centered bold, l=left, p=numbered paragraph
        ("l", f"{atty} (SBN {rng.randint(100000, 399999)})"), ("l", ctx.get("firm").upper()),
        ("l", f"Attorneys for Plaintiff {P.upper()}"),
        ("c", "UNITED STATES DISTRICT COURT"), ("c", f"{'NORTHERN' if rng.random() < 0.5 else 'SOUTHERN'} DISTRICT OF {ctx.state.upper()}"),
        ("l", f"{P.upper()}, Plaintiff, v. {D.upper()}, Defendant."),
        ("b", f"Case No. {rng.randint(1, 9)}:{rng.randint(22, 26)}-cv-{rng.randint(10000, 99999)}"),
        ("c", "COMPLAINT FOR BREACH OF CONTRACT"),
    ]
    facts = [
        "Plaintiff {A} is a {atype} organized under the laws of the State of {state}, with its principal place of business in {city}.",
        "Defendant {B} is a {btype} doing business in {city}, {state}.",
        "This Court has jurisdiction over this action because the amount in controversy exceeds {money}, exclusive of interest and costs.",
        "Venue is proper in this district because a substantial part of the events giving rise to the claim occurred in {city}.",
        "On or about {date}, Plaintiff and Defendant entered into a written Supply Agreement under which Plaintiff agreed to deliver {n} units of {goods} for a total price of {money}.",
        "Plaintiff performed all of its obligations under the Agreement, including delivery of the first shipment on {date}.",
        "Defendant accepted the {goods} without objection and used them in its operations at its {city} facility.",
        "Under Section {sec} of the Agreement, payment of each invoice was due within {days} days of receipt.",
    ]
    cause1 = [
        "Plaintiff incorporates by reference the allegations of the preceding paragraphs as though fully set forth herein.",
        "The Agreement is a valid and enforceable contract supported by adequate consideration.",
        "Defendant breached the Agreement by failing to pay Invoice No. {inv} in the amount of {money}, which became due on {date}.",
        "Despite repeated demands, including written notice sent on {date}, Defendant has refused to pay the outstanding balance.",
        "As a direct and proximate result of Defendant’s breach, Plaintiff has suffered damages in an amount to be proven at trial, but not less than {money}.",
        "Plaintiff is entitled to recover interest on the unpaid amount at the rate of {pct} percent per annum from {date}.",
    ]
    cause2 = [
        "Plaintiff incorporates by reference the allegations of the preceding paragraphs as though fully set forth herein.",
        "Plaintiff sent Defendant monthly statements of account, the most recent of which, dated {date}, showed a balance due of {money}.",
        "Defendant did not object to any statement within a reasonable time, and the balance of {money} became an account stated.",
        "Plaintiff requests that the Court award its reasonable attorneys’ fees and costs as provided in Section {sec} of the Agreement.",
    ]
    more = ["Defendant likewise failed to pay Invoice No. {inv} in the amount of {small}, which became due on {date}.",
            "Plaintiff delivered {n} additional units of {goods} on {date}, and Defendant has paid nothing toward the invoice of {money}.",
            "On {date}, Plaintiff’s counsel sent Defendant a final demand for payment of {money}, to which Defendant did not respond."]
    n_pages = rng.choice([3, 4])
    target = (n_pages - 1) * 28 + rng.randint(8, 16)

    def lines_of(kind, text, num=None):
        font = "Times-Bold" if kind in ("c", "b") else "Times-Roman"
        t = f"{num}. {text}" if num else text
        return font, wrap(t, font, 12, W - 54 - 80, 24 if num else 0)

    # estimate total lines as paragraphs are added
    total = sum(len(lines_of(k, t)[1]) for k, t in blocks)
    num = 0

    def add_p(tpl):
        nonlocal total, num
        num += 1
        text = ctx.fill(tpl)
        blocks.append(("p", text, num))
        total += len(lines_of("p", text, num)[1])

    def add_h(t):
        nonlocal total
        blocks.append(("c", t))
        total += 1

    i = 0
    for t in facts:
        add_p(t)
    add_h("FIRST CAUSE OF ACTION"); add_h("(Breach of Contract)")
    for t in cause1:
        add_p(t)
    while total < target - 14:
        add_p(rng.choice(more))
    add_h("SECOND CAUSE OF ACTION"); add_h("(Account Stated)")
    for t in cause2:
        if total >= target:
            break
        add_p(t)
    add_p("WHEREFORE, Plaintiff prays for judgment against Defendant for damages in the amount of {money}, plus interest, attorneys’ fees and costs of suit.")
    blocks.append(("l", f"Dated: {longdate(rdate(rng, 2024, 2025))}"))
    blocks.append(("l", "Respectfully submitted,"))
    blocks.append(("l", f"{ctx.get('firm').upper()}"))
    blocks.append(("l", f"By: {atty.title()}"))
    blocks.append(("l", f"Attorneys for Plaintiff {P}"))

    # slots: (page_idx, slot, font, text, align, block_idx)
    slots = []
    for bi, b in enumerate(blocks):
        kind, text = b[0], b[1]
        font, ls = lines_of(kind, text, b[2] if kind == "p" else None)
        for ln in ls:
            slots.append((font, ln, "c" if kind in ("c",) else "l", bi, 24 if kind == "p" and ln is ls[0] else 0))
    npages = (len(slots) + 27) // 28
    pages = [Page() for _ in range(npages)]
    title = "COMPLAINT"
    base = rng.randint(100, 9000)
    for pi, p in enumerate(pages):
        p.bates = f"{pfx}{base + pi:06d}"
        chunk = slots[pi * 28:(pi + 1) * 28]
        for i in range(1, 29):
            p.text(60, H - 72 - (i - 1) * 24, str(i), "Times-Roman", 10, "r")
        p.line(66, 40, 66, H - 40)
        p.line(W - 42, 40, W - 42, H - 40, 0.5)
        groups = {}
        for si, (font, ln, al, bi, ind) in enumerate(chunk):
            y = H - 72 - si * 24
            if al == "c":
                p.text((80 + W - 54) / 2, y, ln, font, 12, "c")
            else:
                p.text(80 + ind, y, ln, font, 12)
            groups.setdefault(bi, []).append(ln)
        p.parts = [" ".join(v) for v in groups.values()]
        foot = f"{title} - Page {pi + 1} of {npages}"
        p.text(W / 2, 30, foot, "Times-Roman", 9, "c")
        p.text(W - 44, 30, p.bates, "Times-Roman", 9, "r")
        p.furniture = [foot, p.bates]
    return pages, "Pleading paper: margin line numbers 1-28 are excluded from text (score with --strip-line-numbers)."


# ------------------------------------------------------------------ transcript 4-up

def build_transcript(rng, seed):
    ctx = Ctx(rng)
    witness = ctx.pa
    atty = ctx.rng.choice(LAST).upper()
    opp = ctx.rng.choice([n for n in LAST if n.upper() != atty]).upper()
    qa = [
        ("Please state your full name for the record.", f"{witness}."),
        ("By whom are you employed?", "I work for {a}, in {city}."),
        ("What is your position there?", "I am the {title}."),
        ("Did you sign the Supply Agreement dated {date}?", "Yes, I signed it on behalf of the company."),
        ("Do you recall receiving Invoice No. {inv}?", "I recall receiving it in the first week of {month}."),
        ("What was the amount of that invoice?", "It was {money}."),
        ("Was that amount ever paid?", "No, not to my knowledge."),
        ("When did you first speak with {pb} about the shipment?", "It was sometime around {date}."),
        ("Did anyone at {B} complain about the quality of the {goods}?", "Not before {date}."),
        ("Showing you what has been marked as Exhibit {x}, do you recognize this document?", "Yes, that is the purchase order."),
        ("And what is the date on that purchase order?", "It is dated {date}."),
        ("How many units were delivered on that date?", "I believe it was {n} units."),
        ("Did you ever agree to extend the payment deadline?", "No. The deadline was never extended."),
        ("Who prepared the delivery schedule?", "{pb} prepared it with our logistics department."),
        ("Were you present at the {city} facility when the {goods} arrived?", "I was there that morning."),
        ("Who signed the delivery receipt?", "{pb} signed it."),
        ("Did you ever inspect the {goods} yourself?", "I inspected a sample on {date}."),
        ("Was there any written complaint about the shipment?", "Not that I am aware of."),
        ("What was the agreed price per shipment?", "As I recall, it was {money}."),
        ("Did you discuss the unpaid balance with {pb}?", "Yes, we spoke by telephone on {date}."),
        ("What did {pb} say to you during that call?", "{pb} said the payment would be made by the end of the month."),
        ("Is it your testimony that the full balance of {money} remains outstanding?", "That is correct."),
    ]
    colloquy = [(f"MR. {opp}:", "Objection. Form."), (f"MR. {opp}:", "Objection. Calls for speculation."),
                (f"MR. {atty}:", "Let me rephrase the question."), (f"MR. {atty}:", "Take your time, " + witness + "."),
                ("THE WITNESS:", "Could you repeat the question?")]
    sheets = rng.choice([2, 3])
    first = rng.randint(8, 60) * 4 + 1
    total_pages = sheets * 4
    need = total_pages * 25
    lines = []  # (prefix-free text, ...)
    order = list(range(len(qa)))
    rng.shuffle(order)
    k = 0
    while len(lines) < need:
        q, a = qa[order[k % len(order)]]
        k += 1
        q, a = ctx.fill(q), ctx.fill(a)
        exch = [("Q.", q), ("A.", a)]
        if rng.random() < 0.25:
            sp, t = rng.choice(colloquy)
            exch.insert(1, (sp, t))
        for tag, t in exch:
            ls = wrap(f"{tag} {t}", "Courier", 8, 50 * 4.8)
            lines += [(ln if i == 0 else "   " + ln) for i, ln in enumerate(ls)]
    lines = lines[:need]
    pages = []
    cw, ch = 288, 378
    for s in range(sheets):
        p = Page("transcript")
        parts = []
        for q in range(4):
            pno = first + s * 4 + q
            ox, oy = 18 + (q % 2) * cw, H - 18 - (q // 2) * ch  # top-left of cell
            p.rect(ox + 2, oy - ch + 2, cw - 4, ch - 4)
            p.text(ox + cw / 2, oy - 18, f"Page {pno}", "Courier-Bold", 8, "c")
            part = [f"Page {pno}"]
            for i in range(25):
                t = lines[(s * 4 + q) * 25 + i]
                y = oy - 36 - i * 12.5
                p.text(ox + 24, y, str(i + 1), "Courier", 8, "r")
                p.text(ox + 30, y, t, "Courier", 8)
                part.append(f"{i + 1} {t.strip()}")
            parts.append("\n".join(part))
        p.parts = parts
        pages.append(p)
    return pages, "4-up minuscript, 4 transcript pages per sheet in quadrant reading order TL, TR, BL, BR."


# ------------------------------------------------------------------ fax

def build_fax(rng, seed):
    ctx = Ctx(rng)
    pages = [Page(), Page()]
    header = lambda i: f"FROM: {ctx.get('A').upper()}   TEL {rng.randint(200, 899)}-{rng.randint(200, 899)}-{rng.randint(1000, 9999)}   PAGE {i:02d}/02"
    fl1 = Flow(pages[0], 80, W - 80, H - 70, 60)
    paras = pick_paras(ctx, 10)
    draw_letter(ctx, fl1, paras[:3], sig=False, size=12)
    used = 3
    while used < len(paras) and fl1.para(paras[used], size=12, leading=16, gap=9):
        used += 1
    fl2 = Flow(pages[1], 80, W - 80, H - 70, 60)
    # Page 2 continues with the remaining paragraphs and the signature block.
    rest = paras[used:] or pick_paras(ctx, 3)
    for t in rest[:4]:
        fl2.para(t, size=12, leading=16, gap=9)
    fl2.para("Sincerely,", size=12, gap=18)
    fl2.lines([ctx.pa, rng.choice(TITLES)], gap=0)
    for i, p in enumerate(pages, 1):
        h = header(i)
        p.text(40, H - 28, h, "Courier", 8)
        p.furniture = [h]
    return pages, "Image-only. 200 DPI, skew +/-1.0-1.5 deg, threshold binarized, 0.25% salt-and-pepper, JPEG q=40."


# ------------------------------------------------------------------ table

DESCS = ["Review of Supply Agreement", "Draft demand letter", "Telephone conference with client",
         "Prepare discovery responses", "Analyze invoice history", "Research on account stated",
         "Correspondence with opposing counsel", "Revise settlement proposal", "Attend case management conference",
         "Prepare deposition outline", "Review production documents", "Draft motion for summary judgment"]


def build_table(rng, seed):
    ctx = Ctx(rng)
    cols = [("Invoice No.", 70, "l"), ("Date", 64, "l"), ("Description", 190, "l"), ("Hours", 40, "r"),
            ("Rate", 56, "r"), ("Amount", 72, "r")]
    n1, n2 = rng.randint(21, 24), rng.randint(13, 16)
    d = rdate(rng, 2024, 2024)
    rows, tot_h, tot_a = [], 0.0, 0.0
    inv = rng.randint(2400, 9000)
    for i in range(n1 + n2):
        if rng.random() < 0.5:
            inv += 1
        d += timedelta(days=rng.randint(0, 6))
        h = rng.randint(3, 65) / 10
        rate = rng.choice([275, 325, 350, 425, 525])
        amt = round(h * rate, 2)
        tot_h += h
        tot_a += amt
        rows.append([f"INV-{inv}", d.strftime("%m/%d/%Y"), rng.choice(DESCS), f"{h:.1f}", money(rate), money(amt)])
    totals = ["TOTAL", "", "", f"{tot_h:.1f}", "", money(tot_a)]
    pages = []
    for pi, chunk in enumerate((rows[:n1], rows[n1:] + [totals])):
        p = Page("table")
        p.text(W / 2, H - 60, "SCHEDULE B" + (" (continued)" if pi else ""), "Helvetica-Bold", 13, "c")
        sub = f"Fee Schedule - {ctx.get('A')} - Matter No. {rng.randint(10000, 99999)}"
        p.text(W / 2, H - 78, sub, "Helvetica", 10, "c")
        p.parts.append("SCHEDULE B" + (" (continued)" if pi else ""))
        p.parts.append(sub)
        y, x0, rh = H - 112, 60, 18.5
        trs = [[c[0] for c in cols]] + chunk
        for ri, row in enumerate(trs):
            x = x0
            for (name, cw, al), val in zip(cols, row):
                p.rect(x, y - 5, cw, rh)
                font = "Helvetica-Bold" if ri == 0 or row is totals else "Helvetica"
                assert stringWidth(val, font, 8.5) < cw - 8, val
                if al == "l":
                    p.text(x + 4, y, val, font, 8.5)
                else:
                    p.text(x + cw - 4, y, val, font, 8.5, "r")
                x += cw
            y -= rh
        p.parts.append("\n".join(" | ".join(r) for r in trs))
        foot = f"Schedule B - Page {pi + 1} of 2"
        p.text(W / 2, 36, foot, "Helvetica", 9, "c")
        p.furniture = [foot]
        pages.append(p)
    return pages, "Page 1 clean vector; page 2 rasterized at 300 DPI with the spike scan recipe (skew 0.5, speckle, blur, JPEG q=55)."


# ------------------------------------------------------------------ production

PROD_ITEMS = [("Hydraulic valve, 2 in.", 118.5), ("Stainless steel fitting kit", 64.25), ("Conveyor roller assembly", 212.0),
              ("Circuit board, rev C", 87.4), ("Refrigeration compressor", 940.0), ("Industrial pump seal", 33.75),
              ("Steel fastener pack (500)", 46.9), ("Packaging film roll", 28.6)]


def prod_letter(ctx, rng):
    p = Page()
    fl = Flow(p, 80, W - 80, H - 80, 70)
    draw_letter(ctx, fl, pick_paras(ctx, rng.randint(2, 3)))
    return p


def prod_email(ctx, rng):
    p = Page()
    fl = Flow(p, 60, W - 60, H - 70, 70)
    nm = lambda x: x.lower().replace(" ", ".")
    hdr = [f"From: {ctx.pa} <{nm(ctx.pa)}@{ctx.get('a').lower()}.com>", f"To: {ctx.pb} <{nm(ctx.pb)}@{ctx.get('b').lower()}.com>",
           f"Date: {longdate(rdate(rng, 2022, 2025))}", f"Subject: Invoice {ctx.get('inv')} and delivery status"]
    fl.lines(hdr, "Helvetica", 10, 14, gap=14)
    fl.para(f"Hi {ctx.pb.split()[0]},", "Helvetica", 10, 14, gap=8)
    for t in pick_paras(ctx, rng.randint(2, 3)):
        fl.para(t, "Helvetica", 10, 14, gap=8)
    fl.para("Thanks,", "Helvetica", 10, 14, gap=2)
    fl.para(ctx.pa, "Helvetica", 10, 14, gap=0)
    return p


def prod_contract(ctx, rng):
    p = Page()
    fl = Flow(p, 72, W - 72, H - 80, 70)
    no = rng.randint(2, 12)
    title, secs = article_sections(ctx, no)
    fl.para(f"ARTICLE {no} \u2014 {title}", "Times-Bold", 12, align="c", gap=10)
    for s in secs:
        fl.para(s, size=11, leading=14, gap=8)
    return p


def prod_invoice(ctx, rng):
    p = Page("table")
    fl = Flow(p, 60, W - 60, H - 70, 70)
    fl.para("INVOICE", "Helvetica-Bold", 16, align="c", gap=10)
    fl.lines([ctx.get("A"), f"{ctx.street()}, {ctx.city}, {ctx.ab} {ctx.zip()}"], "Helvetica", 10, 13, gap=10)
    fl.lines([f"Invoice No. {ctx.get('inv')}", f"Date: {longdate(rdate(rng, 2022, 2025))}", f"Bill To: {ctx.get('B')}"], "Helvetica", 10, 13, gap=14)
    cols = [("Item", 220, "l"), ("Qty", 50, "r"), ("Unit Price", 90, "r"), ("Amount", 100, "r")]
    rows, tot = [], 0.0
    for name, price in rng.sample(PROD_ITEMS, rng.randint(4, 7)):
        q = rng.randint(2, 400)
        tot += q * price
        rows.append([name, str(q), money(price), money(q * price)])
    tax = round(tot * 0.0825, 2)
    rows += [["Subtotal", "", "", money(tot)], ["Sales Tax (8.25%)", "", "", money(tax)], ["TOTAL DUE", "", "", money(tot + tax)]]
    trs = [[c[0] for c in cols]] + rows
    y, x0 = fl.y, 72
    for ri, row in enumerate(trs):
        x = x0
        for (name, cw, al), val in zip(cols, row):
            p.rect(x, y - 5, cw, 20)
            font = "Helvetica-Bold" if ri == 0 or ri >= len(trs) - 3 else "Helvetica"
            if al == "l":
                p.text(x + 4, y, val, font, 9.5)
            else:
                p.text(x + cw - 4, y, val, font, 9.5, "r")
            x += cw
        y -= 20
    p.parts.append("\n".join(" | ".join(r) for r in trs))
    fl.y = y - 10
    fl.para(ctx.fill("Payment is due within {days} days of the invoice date. Late balances accrue interest at {pct} percent per annum."), "Helvetica", 9.5, 13)
    return p


def prod_scanned(ctx, rng):
    p = Page()
    fl = Flow(p, 80, W - 80, H - 80, 70)
    fl.para("MEMORANDUM", "Times-Bold", 14, align="c", gap=12)
    fl.lines([f"TO: {ctx.pb}", f"FROM: {ctx.pa}", f"DATE: {longdate(rdate(rng, 2022, 2025))}",
              f"RE: Shipment of {ctx.get('goods')} to {ctx.get('B')}"], "Times-Roman", 11, 15, gap=14)
    for t in pick_paras(ctx, rng.randint(3, 4)):
        fl.para(t, size=11, leading=15, gap=9)
    return p


def build_production(rng, seed, n=500):
    templates = [prod_letter, prod_email, prod_contract, prod_invoice, prod_scanned]
    pages = []
    for i in range(n):
        ctx = Ctx(rng)
        p = templates[i % 5](ctx, rng)
        p.bates = f"ACME{i + 1:07d}"
        p.text(W - 40, 30, p.bates, "Helvetica", 9, "r")
        p.furniture = [p.bates]
        p.scanned = i % 5 == 4
        pages.append(p)
    return pages, "500 pages cycling letter/email/contract/invoice/scanned; every fifth page (20%) is rasterized at 200 DPI with the spike scan recipe."


# ------------------------------------------------------------------ writers

def write_vector(pages, path):
    c = canvas.Canvas(path, pagesize=letter, invariant=1)
    for p in pages:
        render_page(c, p)
    c.save()


def write_family(fam, seed, out):
    rng = random.Random(f"{seed}:{fam}")
    path = os.path.join(out, f"{fam}.pdf")
    if fam in ("pleading", "contract", "transcript4up"):
        pages, notes = {"pleading": build_pleading, "contract": build_contract, "transcript4up": build_transcript}[fam](rng, seed)
        write_vector(pages, path)
    elif fam == "fax":
        pages, notes = build_fax(rng, seed)
        c = canvas.Canvas(path, pagesize=letter, invariant=1)
        for i, p in enumerate(pages):
            skew = rng.choice([-1, 1]) * rng.uniform(1.0, 1.5)
            draw_image_page(c, fax_recipe(rasterize(p, 200), seed * 100 + i, skew))
        c.save()
    elif fam == "table":
        pages, notes = build_table(rng, seed)
        c = canvas.Canvas(path, pagesize=letter, invariant=1)
        render_page(c, pages[0])
        draw_image_page(c, scan_recipe(rasterize(pages[1], 300), seed * 100 + 1))
        c.save()
    elif fam == "production":
        pages, notes = build_production(rng, seed)
        c = canvas.Canvas(path, pagesize=letter, invariant=1)
        for i, p in enumerate(pages):
            if p.scanned:
                draw_image_page(c, scan_recipe(rasterize(p, 200), seed * 1000 + i))
            else:
                render_page(c, p)
        c.save()
    else:
        raise SystemExit(f"unknown family {fam}")
    gt = {"family": fam, "seed": seed, "pages": [gt_page(p) for p in pages], "notes": notes}
    with open(os.path.join(out, f"{fam}.gt.json"), "w") as f:
        json.dump(gt, f, indent=1, ensure_ascii=False)
    return len(pages)


FAMILIES = ["pleading", "contract", "transcript4up", "fax", "table", "production"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", type=int, required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--families", default=",".join(FAMILIES))
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    for fam in a.families.split(","):
        t = time.time()
        n = write_family(fam, a.seed, a.out)
        print(f"{fam}: {n} pages in {time.time() - t:.1f}s")


if __name__ == "__main__":
    main()
