// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "VLCPiP",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(name: "VLCPiP", path: "Sources/VLCPiP")
    ]
)
