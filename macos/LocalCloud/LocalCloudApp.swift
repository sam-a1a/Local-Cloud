import SwiftUI

@main
struct LocalCloudApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var mesh = Mesh()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environment(mesh)
                .frame(minWidth: 820, minHeight: 520)
                .task {
                    delegate.mesh = mesh
                    mesh.begin()
                }
        }
        .commands {
            CommandGroup(replacing: .newItem) {}
        }
    }
}

/// Exists for one thing: to stop the engine before the process does.
///
/// SwiftUI has no scene event for "about to quit", and this is not tidiness.
/// Stopping unregisters this device from mDNS, and a goodbye that never gets
/// sent leaves it advertised on the network until every other device times it
/// out - so a peer keeps offering to send files to a Mac that is not there.
/// `terminateLater` is the only way to hold the process open long enough.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    var mesh: Mesh?

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let mesh else { return .terminateNow }
        Task {
            await mesh.end()
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }

    /// Closing the window quits, because there is nowhere else for this app to
    /// be. Android earned a background mode by way of a foreground service and
    /// a notification; the Mac's version of that is a menu bar item, and until
    /// there is one, a window that closes into nothing would be a device that
    /// quietly stopped syncing and never said.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }
}
