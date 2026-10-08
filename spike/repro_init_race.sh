#!/bin/zsh
# Repro for the Apple Vision concurrent start-up crash (SIGSEGV in objc_retain under
# TextRecognition; docs/spike.md). Launches fresh processes, each starting with many
# simultaneous requests, and counts crashes: exit 139 (SIGSEGV), 138 (SIGBUS), 132 (SIGILL),
# 133 (SIGTRAP). Every other non-zero exit is recorded too.
# In scan mode the supervisor hides crashes behind a retry, so it also counts the
# "Apple Vision crashed" notes on stderr (crashes the supervisor contained).
# usage: spike/repro_init_race.sh [iterations=300] [threads=10] [mode=bench|scan]
N=${1:-300}; T=${2:-10}; MODE=${3:-bench}
ROOT=${0:A:h:h}
FX=$ROOT/spike/fixtures
W=$(mktemp -d)
crashes=0; contained=0; launches_with_contained=0; other=0; codes=""
for i in $(seq $N); do
  if [[ $MODE == bench ]]; then
    $ROOT/target/release/examples/vision_bench $FX/legal_300dpi.png $T >/dev/null 2>$W/err
  else
    rm -rf $W/c $W/s $W/three_pages.md $W/three_pages.pdf $W/three_pages.md.ocr-partial; mkdir -p $W/c $W/s; cp $FX/three_pages.pdf $W/
    (cd $W && OCR_CONFIG_DIR=$W/c OCR_STATE_DIR=$W/s $ROOT/target/release/ocr scan \
      --input-file three_pages.pdf --jobs $T --overwrite --quiet) >/dev/null 2>$W/err
  fi
  rc=$?
  n=$(grep -c "Apple Vision crashed" $W/err)
  if (( n > 0 )); then ((contained += n)); ((launches_with_contained++)); fi
  case $rc in
    0) ;;
    139|138|132|133) ((crashes++)); codes="$codes $rc" ;;
    *) ((other++)); codes="$codes $rc" ;;
  esac
done
rm -rf $W
echo "mode=$MODE threads=$T launches=$N crashed_exits=$crashes contained_crashes=$contained (in $launches_with_contained launches) other_nonzero=$other (codes:$codes)"
