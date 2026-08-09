import SwiftUI

/// The three things the engine has an opinion about: what is in the catalog,
/// which devices are in the mesh, and what is on its way out of it.
enum Screen: String, CaseIterable, Identifiable {
    case files = "Files"
    case devices = "Devices"
    case trash = "Trash"

    var id: Self { self }

    var symbol: String {
        switch self {
        case .files: "shippingbox"
        case .devices: "laptopcomputer.and.iphone"
        case .trash: "trash"
        }
    }
}

struct ContentView: View {
    @Environment(Mesh.self) private var mesh
    @State private var screen: Screen = .files

    var body: some View {
        NavigationSplitView {
            sidebar
        } detail: {
            detail
                .safeAreaInset(edge: .bottom, spacing: 0) {
                    if let notice = mesh.notice {
                        NoticeBar(notice: notice) { mesh.dismissNotice() }
                            .transition(.move(edge: .bottom))
                    }
                }
                .animation(.default, value: mesh.notice?.id)
        }
        .navigationTitle("LocalCloud")
    }

    private var sidebar: some View {
        VStack(spacing: 0) {
            List(Screen.allCases, selection: $screen) { screen in
                Label(screen.rawValue, systemImage: screen.symbol)
                    .badge(badge(for: screen))
                    .tag(screen)
            }
            .listStyle(.sidebar)

            Divider()
            ThisDeviceCard()
        }
        .navigationSplitViewColumnWidth(min: 200, ideal: 230)
    }

    @ViewBuilder
    private var detail: some View {
        if let fatal = mesh.state.fatal {
            ContentUnavailableView {
                Label("The engine could not start", systemImage: "bolt.horizontal.circle")
            } description: {
                Text(fatal).textSelection(.enabled)
            }
        } else if !mesh.state.ready {
            ProgressView("Starting the engine…")
        } else {
            switch screen {
            case .files: FilesScreen()
            case .devices: DevicesScreen()
            case .trash: TrashScreen()
            }
        }
    }

    private func badge(for screen: Screen) -> Int {
        switch screen {
        case .files: mesh.state.items.count
        // Something is waiting for an answer: another device has asked to pair.
        case .devices: mesh.state.offers.count
        case .trash: mesh.state.trash.count
        }
    }
}

/// Who this Mac is on the mesh, and whether it is on it at all.
///
/// Bottom of the sidebar, visible from every screen, because every question on
/// every other screen is asked relative to this device.
struct ThisDeviceCard: View {
    @Environment(Mesh.self) private var mesh
    @State private var editing = false
    @State private var draft = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                ReachabilityDot(reachable: mesh.state.running)
                if editing {
                    TextField("Name", text: $draft)
                        .textFieldStyle(.roundedBorder)
                        .onSubmit(commit)
                } else {
                    Text(mesh.state.thisDevice.name.isEmpty
                         ? "This Mac" : mesh.state.thisDevice.name)
                        .fontWeight(.medium)
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    Button("Rename", systemImage: "pencil") {
                        draft = mesh.state.thisDevice.name
                        editing = true
                    }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .help("Rename this device")
                }
            }
            Text("\(mesh.state.thisDevice.platform) · \(mesh.state.thisDevice.shortId)")
                .font(.caption)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
            Text(mesh.state.running ? "On the mesh" : "Not running")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func commit() {
        editing = false
        let name = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name != mesh.state.thisDevice.name else { return }
        mesh.rename(to: name)
    }
}
