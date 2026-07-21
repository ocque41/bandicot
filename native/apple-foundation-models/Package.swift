// swift-tools-version: 6.2

import PackageDescription

let package = Package(
    name: "BandicotAppleFoundationModels",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "bandicot-apple-foundation-models", targets: ["BandicotAppleFoundationModels"])
    ],
    targets: [
        .target(name: "BandicotFoundationModelsBridgeCore"),
        .executableTarget(
            name: "BandicotAppleFoundationModels",
            dependencies: ["BandicotFoundationModelsBridgeCore"]
        ),
        .testTarget(
            name: "BandicotFoundationModelsBridgeCoreTests",
            dependencies: ["BandicotFoundationModelsBridgeCore"]
        )
    ]
)
