// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "OcrBridge",
    platforms: [.macOS("26.0")],
    products: [
        .library(name: "OcrBridge", type: .static, targets: ["OcrBridge"])
    ],
    targets: [
        .target(name: "OcrBridge")
    ]
)
