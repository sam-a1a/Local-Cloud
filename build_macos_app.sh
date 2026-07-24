#!/bin/bash
set -e

echo "🦀 Building Rust for macOS (Universal Binary)..."
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

mkdir -p target/universal-macos
lipo -create \
    target/aarch64-apple-darwin/release/liblocalcloud.a \
    target/x86_64-apple-darwin/release/liblocalcloud.a \
    -output target/universal-macos/liblocalcloud.a

echo "🔧 Generating Swift bindings via UniFFI..."
cargo run --bin uniffi_bindgen generate \
    --library target/aarch64-apple-darwin/release/liblocalcloud.dylib \
    --language swift \
    --out-dir ./target/swift_bindings

echo "📦 Setting up Xcode project structure..."
APP_DIR="./macos_app"
rm -rf $APP_DIR
mkdir -p $APP_DIR/RustLibs
mkdir -p $APP_DIR/Sources

# Copy Rust artifacts
cp target/universal-macos/liblocalcloud.a $APP_DIR/RustLibs/
cp target/swift_bindings/localcloudFFI.h $APP_DIR/RustLibs/
# Xcode requires the modulemap to be named exactly `module.modulemap` to auto-discover it
cp target/swift_bindings/localcloudFFI.modulemap $APP_DIR/RustLibs/module.modulemap
cp target/swift_bindings/localcloud.swift $APP_DIR/Sources/

# Create the SwiftUI App file
cat <<'SWIFT_EOF' > $APP_DIR/Sources/LocalCloudApp.swift
import SwiftUI
import localcloud

@main
struct LocalCloudApp: App {
    let engine: Engine

    init() {
        do {
            let fileManager = FileManager.default
            let appSupportDir = try fileManager.url(
                for: .applicationSupportDirectory,
                in: .userDomainMask,
                appropriateFor: nil,
                create: true
            )
            
            let baseDir = appSupportDir.appendingPathComponent("LocalCloudData").path
            let syncDir = appSupportDir.appendingPathComponent("SyncFolder").path
            
            try? fileManager.createDirectory(atPath: baseDir, withIntermediateDirectories: true)
            try? fileManager.createDirectory(atPath: syncDir, withIntermediateDirectories: true)

            engine = try Engine(baseDir: baseDir, syncDirPath: syncDir)
            try engine.start()
            
            print("Engine started: \(engine.deviceShortId())")
            
            // Poll events in background so we don't block UI
            DispatchQueue.global(qos: .background).async {
                while true {
                    if let event = engine.pollEvent(timeoutMs: 500) {
                        print("[Event]: \(event)")
                    }
                }
            }
        } catch {
            fatalError("Failed to init engine: \(error)")
        }
    }

    var body: some Scene {
        WindowGroup {
            VStack {
                Text("LocalCloud Engine Running")
                    .font(.title)
                    .padding()
                Text("Device ID: \(engine.deviceShortId())")
                    .font(.mono())
            }
            .frame(width: 400, height: 200)
        }
    }
}
SWIFT_EOF

# Create XcodeGen configuration
cat <<YAML_EOF > $APP_DIR/project.yml
name: LocalCloudMac
options:
  bundleIdPrefix: com.localcloud
targets:
  LocalCloudMac:
    type: application
    platform: macOS
    deploymentTarget: "12.0"
    sources:
      - path: Sources
      - path: RustLibs
    settings:
      base:
        SWIFT_INCLUDE_PATHS: \$(SRCROOT)/RustLibs
        OTHER_LDFLAGS: -llocalcloud -framework Security -framework CoreFoundation -framework SystemConfiguration
        LIBRARY_SEARCH_PATHS: \$(SRCROOT)/RustLibs
YAML_EOF

echo "⚙️ Checking for XcodeGen..."
if ! command -v xcodegen &> /dev/null
then
    echo "XcodeGen not found. Installing via Homebrew..."
    brew install xcodegen
fi

echo "🏗️ Generating Xcode Project..."
cd $APP_DIR
xcodegen generate

echo "🚀 Opening Xcode..."
open LocalCloudMac.xcodeproj
