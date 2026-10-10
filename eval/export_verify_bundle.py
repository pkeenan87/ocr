"""Copy a run of `eval/verify_eval.py` into a committable bundle for `eval/azure_verify_eval.py`.

usage:
    PY -I eval/export_verify_bundle.py eval/out/verify/apple-all eval/verify_bundle/macos27/apple-all

Copies `blind/<id>.md`, `blind/<id>.verify.json` and `truth/<id>.verify.json`, and reduces each
report's `markdown` path to the file name, so no local path is committed.
"""

import glob
import json
import os
import shutil
import sys


def main():
    src, dst = sys.argv[1:3]
    n = 0
    for kind, pattern in [("blind", "*.md"), ("blind", "*.verify.json"), ("truth", "*.verify.json")]:
        os.makedirs(os.path.join(dst, kind), exist_ok=True)
        for f in sorted(glob.glob(os.path.join(src, kind, pattern))):
            out = os.path.join(dst, kind, os.path.basename(f))
            if f.endswith(".verify.json"):
                with open(f, encoding="utf-8") as fh:
                    d = json.load(fh)
                d["markdown"] = os.path.basename(d["markdown"])
                with open(out, "w", encoding="utf-8", newline="\n") as fh:
                    json.dump(d, fh, indent=1, ensure_ascii=False)
                    fh.write("\n")
            else:
                shutil.copyfile(f, out)
            n += 1
    print(f"{n} files to {dst}")


if __name__ == "__main__":
    main()
