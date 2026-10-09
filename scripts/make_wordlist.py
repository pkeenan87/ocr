#!/usr/bin/env python3
"""Builds data/words-en.txt, the English word list behind the suspect-page check
(src/suspect.rs), from pyspellchecker's English dictionary.

    pip download pyspellchecker==0.9.0 --no-deps -d /tmp/wl
    python3 -I scripts/make_wordlist.py /tmp/wl/pyspellchecker-0.9.0-py3-none-any.whl

Keeps lowercase alphabetic words of 4-20 letters (shorter words are never checked), sorted
and front-coded: each line is the number of leading letters shared with the previous word
(0-9) followed by the rest of the word.
"""

import gzip
import io
import json
import re
import sys
import zipfile

HEADER = """\
# English words for the suspect-page check (src/suspect.rs); built by scripts/make_wordlist.py.
# Front-coded: a line is the count of letters shared with the previous word, then the rest.
# Source: pyspellchecker 0.9.0, spellchecker/resources/en.json.gz, under the MIT License:
#
# Copyright (c) 2018-2021 Tyler Barrus
#
# Permission is hereby granted, free of charge, to any person obtaining a copy of this
# software and associated documentation files (the "Software"), to deal in the Software
# without restriction, including without limitation the rights to use, copy, modify, merge,
# publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons
# to whom the Software is furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in all copies or
# substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
# INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
# PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE
# FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
# OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
# DEALINGS IN THE SOFTWARE.
"""


def main(wheel: str) -> None:
    with zipfile.ZipFile(wheel) as z:
        raw = z.read("spellchecker/resources/en.json.gz")
    words = json.load(gzip.open(io.BytesIO(raw)))
    keep = sorted(w for w in words if re.fullmatch(r"[a-z]{4,20}", w))
    lines, prev = [], ""
    for w in keep:
        n = 0
        while n < min(len(prev), len(w), 9) and prev[n] == w[n]:
            n += 1
        lines.append(f"{n}{w[n:]}")
        prev = w
    with open("data/words-en.txt", "w", newline="\n") as f:
        f.write(HEADER)
        f.write("\n".join(lines))
        f.write("\n")
    print(f"{len(keep)} words")


if __name__ == "__main__":
    main(sys.argv[1])
