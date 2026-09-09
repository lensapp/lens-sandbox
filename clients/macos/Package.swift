// swift-tools-version: 5.9
import PackageDescription

var products: [Product] = [.library(name: "LNSClient", targets: ["LNSClient"])]
var targets: [Target] = [
    .target(name: "LNSClient"),
    .testTarget(name: "LNSClientTests", dependencies: ["LNSClient"], resources: [.copy("Fixtures")])
]
#if os(macOS)
products.append(.executable(name: "LNS", targets: ["LNSMac"]))
targets.append(.executableTarget(name: "LNSMac", dependencies: ["LNSClient"]))
#endif

let package = Package(
    name: "LNSMac",
    platforms: [.macOS(.v13)],
    products: products,
    targets: targets
)
