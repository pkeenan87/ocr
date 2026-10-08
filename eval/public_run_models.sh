#!/usr/bin/env bash
# Stage B of the public court-filing set (eval/PUBLIC_SET.md): scan the 20 documents of
# eval/public_set.json with one model engine, into eval/out/public/<engine>/<id>.md.
#
# usage: eval/public_run_models.sh <paddleocr-vl|olmocr|dots-ocr> [--setup] [--port N]
#
#   --setup   first run `ocr server setup --model <engine> --non-interactive` in the isolated state
#             (needed once per engine; weights come from the HF cache).
#   --port    server port (default 8091, NOT 8080, so it cannot collide with another eval's server).
#
# Uses the ISOLATED state eval/out/public/state/{config,state}, never ~/.config/ocr. Starts the
# server, scans each document with the page range from the manifest, stops the server (also on
# Ctrl-C or error). Per-doc wall time and exit code go to eval/out/public/<engine>/runs.tsv.
# Exit 1 (flagged pages) still counts as a result; exit 3 means the server died: re-run to resume.
# Then judge: spike/.venv/bin/python -I eval/public_compare.py
set -euo pipefail

ENGINE="${1:-}"
case "$ENGINE" in paddleocr-vl|olmocr|dots-ocr) ;; *)
  echo "usage: $0 <paddleocr-vl|olmocr|dots-ocr> [--setup] [--port N]" >&2; exit 2 ;;
esac
shift
SETUP=0; PORT=8091
while [ $# -gt 0 ]; do
  case "$1" in
    --setup) SETUP=1 ;;
    --port) PORT="$2"; shift ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift
done

REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"
OCR="$REPO/target/release/ocr"
PY="$REPO/spike/.venv/bin/python"
PUB="$REPO/eval/out/public"
OUT="$PUB/$ENGINE"
export OCR_CONFIG_DIR="$PUB/state/config" OCR_STATE_DIR="$PUB/state/state"
mkdir -p "$OUT" "$OCR_CONFIG_DIR" "$OCR_STATE_DIR"

if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "port $PORT is already in use; pick another with --port" >&2; exit 2
fi

if [ "$SETUP" = 1 ]; then
  "$OCR" server setup --model "$ENGINE" --non-interactive
fi

stop_server() { "$OCR" server stop || true; }
trap stop_server EXIT
"$OCR" server start --model "$ENGINE" --port "$PORT"

# id <TAB> pages argument ("-" when the whole document is used)
"$PY" -I -c '
import json, sys
for d in json.load(open(sys.argv[1]))["documents"]:
    print(d["id"] + "\t" + (d["pages_arg"] or "-"))
' "$REPO/eval/public_set.json" > "$OUT/.docs.tsv"

printf "id\texit\twall_s\n" > "$OUT/runs.tsv"
while IFS=$'\t' read -r id pages; do
  pdf="$PUB/pdfs/$id.pdf"
  if [ ! -s "$pdf" ]; then echo "missing $pdf (download it from pdf_url in the manifest)" >&2; continue; fi
  args=(scan --input-file "$pdf" --output-file "$OUT/$id.md" --model "$ENGINE")
  [ "$pages" != "-" ] && args+=(--pages "$pages")
  t0=$("$PY" -I -c 'import time; print(time.time())')
  set +e
  "$OCR" "${args[@]}"
  rc=$?
  set -e
  t1=$("$PY" -I -c 'import time; print(time.time())')
  wall=$("$PY" -I -c 'import sys; print(round(float(sys.argv[2]) - float(sys.argv[1]), 1))' "$t0" "$t1")
  printf "%s\t%s\t%s\n" "$id" "$rc" "$wall" | tee -a "$OUT/runs.tsv"
  if [ "$rc" = 3 ]; then echo "server unavailable (exit 3); re-run this script to resume" >&2; exit 3; fi
done < "$OUT/.docs.tsv"
echo "done: $OUT"
