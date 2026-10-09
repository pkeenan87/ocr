use std::path::PathBuf;
use std::process::Command;

fn main() {
    // The Swift bridge to Apple Vision exists only on macOS (PLAN §9). `OCR_SKIP_BRIDGE`
    // skips it for `cargo check` of the macOS target from another OS (scripts/xcheck.sh).
    println!("cargo:rerun-if-env-changed=OCR_SKIP_BRIDGE");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos")
        || std::env::var_os("OCR_SKIP_BRIDGE").is_some()
    {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let bridge = manifest.join("bridge");

    println!("cargo:rerun-if-changed=bridge/Package.swift");
    println!("cargo:rerun-if-changed=bridge/Sources");

    let swift_build = |extra: &[&str]| {
        Command::new("swift")
            .args(["build", "-c", "release", "--package-path"])
            .arg(&bridge)
            .args(extra)
            .output()
            .expect("failed to run `swift build` (are the Xcode command line tools installed?)")
    };
    let out = swift_build(&[]);
    assert!(
        out.status.success(),
        "swift build of bridge failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bin_path = swift_build(&["--show-bin-path"]);
    let lib_dir = PathBuf::from(String::from_utf8(bin_path.stdout).unwrap().trim());
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=OcrBridge");

    // Swift runtime + concurrency: link against the toolchain copies, load the OS copies at runtime.
    let swiftc = Command::new("xcrun")
        .args(["--find", "swiftc"])
        .output()
        .unwrap();
    let swiftc = PathBuf::from(String::from_utf8(swiftc.stdout).unwrap().trim());
    let toolchain_lib = swiftc
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("lib/swift/macosx");
    println!("cargo:rustc-link-search=native={}", toolchain_lib.display());
    let sdk = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .unwrap();
    let sdk = String::from_utf8(sdk.stdout).unwrap();
    println!(
        "cargo:rustc-link-search=native={}/usr/lib/swift",
        sdk.trim()
    );
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    for fw in [
        "Foundation",
        "Vision",
        "CoreGraphics",
        "CoreML",
        "ImageIO",
        "PDFKit",
    ] {
        println!("cargo:rustc-link-lib=framework={fw}");
    }
}
