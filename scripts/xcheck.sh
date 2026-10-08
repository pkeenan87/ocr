#!/usr/bin/env bash
# Type-checks ocr for Windows and macOS from Linux (PLAN §9). Uses an official rustc with
# official std builds for both targets, cached in ~/.cache/ocr-xcheck; distro-built rustc
# (e.g. Arch's) rejects official std. The macOS check skips the Swift bridge, so it only
# proves the Rust compiles: Mac tests still need a Mac.
set -euo pipefail

VERSION="${RUST_XCHECK_VERSION:-$(rustc --version | awk '{print $2}')}"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/ocr-xcheck/$VERSION"
TC="$CACHE/toolchain"
HOST=x86_64-unknown-linux-gnu
TARGETS=(x86_64-pc-windows-msvc aarch64-apple-darwin)

# install COMPONENT TARGET: unpacks the official COMPONENT-VERSION-TARGET tarball into $TC.
install() {
    local tarball="$1-$VERSION-$2"
    mkdir -p "$CACHE"
    curl -sSfL "https://static.rust-lang.org/dist/$tarball.tar.xz" | tar xJ -C "$CACHE"
    "$CACHE/$tarball/install.sh" --prefix="$TC" --disable-ldconfig >/dev/null
    rm -rf "${CACHE:?}/$tarball"
}

[ -x "$TC/bin/rustc" ] || install rustc "$HOST"
for t in "$HOST" "${TARGETS[@]}"; do
    [ -f "$TC/lib/rustlib/manifest-rust-std-$t" ] || install rust-std "$t"
done

cd "$(dirname "$0")/.."
for t in "${TARGETS[@]}"; do
    echo "== cargo check --target $t"
    OCR_SKIP_BRIDGE=1 RUSTC="$TC/bin/rustc" CARGO_TARGET_DIR=target/xcheck \
        cargo check --target "$t" --all-targets "$@"
done
