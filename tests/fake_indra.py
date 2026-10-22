#!/usr/bin/python3
"""Stand-in for src/indra_helper.py in tests (OCR_INDRA_CMD). Same argv; no models.

Prints one JSON line per requested page, as the helper does: a body paragraph, a page-footer
block "COMPLAINT - Page N of 3" and, on odd pages, a Bates footer block (SMITH000123,
SMITH000124, ...). Page 2 also holds an HTML table and a `![](picture-0)` placeholder (Picture block).

Env: INDRA_FAKE_LOG appends one JSON line with the argv of each run; INDRA_FAKE_FAIL_PAGE=N
makes page N an error page; INDRA_FAKE_EXIT_AFTER=K ends the run (exit 1, stderr message) after
K pages. INDRA_FAKE_STAMP=1 adds the court's filing stamp as a page-header block on every page.
The argv log shows --pages, --threads and --dpi.
"""

import json
import os
import sys


def main():
    argv = sys.argv[1:]
    opts = {"input": argv[0]}
    i = 1
    while i < len(argv):
        opts[argv[i].lstrip("-")] = argv[i + 1]
        i += 2
    log = os.environ.get("INDRA_FAKE_LOG")
    if log:
        with open(log, "a") as f:
            f.write(json.dumps({"argv": argv, "offline": os.environ.get("HF_HUB_OFFLINE")}) + "\n")
    pages = [int(p) for p in opts["pages"].split(",")]
    fail = int(os.environ.get("INDRA_FAKE_FAIL_PAGE", "0"))
    exit_after = int(os.environ.get("INDRA_FAKE_EXIT_AFTER", "0"))
    stamp = os.environ.get("INDRA_FAKE_STAMP") == "1"
    for done, n in enumerate(pages):
        if exit_after and done >= exit_after:
            sys.stderr.write("fake indra: crashed\n")
            return 1
        if n == fail:
            out = {"page": n, "markdown": "", "blocks": [], "source": "error",
                   "error": "fake indra failure"}
        else:
            md = [f"Fake indra body of page {n}."]
            blocks = [{"label": "Text", "bbox": [100, 200, 900, 260], "text": md[0]}]
            if stamp:
                header = f"Case 1:19-cv-07208-ST Document 31 Filed 02/24/21 Page {n} of 3"
                md.insert(0, header)
                blocks.insert(0, {"label": "Page-Header", "bbox": [100, 20, 1500, 50], "text": header})
            if n == 2:
                md.append("<table><tr><th>Item</th><th>Qty</th></tr><tr><td>Widget</td><td>3</td></tr></table>")
                blocks.append({"label": "Table", "bbox": [100, 300, 900, 500], "text": "Item Qty Widget 3"})
                md.append("![](picture-0)")
                blocks.append({"label": "Picture", "bbox": [100, 520, 900, 800], "text": ""})
            footer = f"COMPLAINT - Page {n} of 3"
            md.append(footer)
            blocks.append({"label": "Page-Footer", "bbox": [100, 1900, 700, 1930], "text": footer})
            if n % 2 == 1:
                bates = f"SMITH{122 + n:06d}"
                md.append(bates)
                blocks.append({"label": "Page-Footer", "bbox": [100, 1950, 400, 1980], "text": bates})
            out = {"page": n, "markdown": "\n\n".join(md), "blocks": blocks,
                   "source": "ocr", "error": None}
        sys.stdout.write(json.dumps(out) + "\n")
        sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
