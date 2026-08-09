import SwiftUI
import AppKit
import UniformTypeIdentifiers

/// The shared catalog: every item in the mesh, and which devices hold it.
///
/// An item is in the catalog whether or not this device has its contents, which
/// is the whole idea - so every row has to say which of the two it is before it
/// says anything else.
struct FilesScreen: View {
    @Environment(Mesh.self) private var mesh
    @State private var choosingFiles = false
    @State private var dropTargeted = false

    var body: some View {
        VStack(spacing: 0) {
            if !mesh.state.collisions.isEmpty {
                CollisionBanner(collisions: mesh.state.collisions)
                Divider()
            }

            if mesh.state.items.isEmpty {
                ContentUnavailableView {
                    Label("Nothing in the catalog", systemImage: "shippingbox")
                } description: {
                    Text("Add a file, or drop one here. Anything put in the sync folder is picked up on its own.")
                } actions: {
                    Button("Add Files…") { choosingFiles = true }
                }
            } else {
                List {
                    ForEach(mesh.state.items) { item in
                        ItemRow(item: item)
                    }
                    if !mesh.state.deleteRequests.isEmpty {
                        Text(deleteRequestSummary)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .listStyle(.inset)
            }
        }
        .toolbar {
            ToolbarItem {
                Button("Add Files…", systemImage: "plus") { choosingFiles = true }
            }
            ToolbarItem {
                Button("Sync Folder", systemImage: "folder") {
                    NSWorkspace.shared.open(URL(fileURLWithPath: mesh.state.syncDirectory))
                }
                .help("Open the folder this device keeps its copies in")
            }
        }
        // A window you can drop a file on is the Mac's share sheet. It reaches
        // the same `import_file` the button does, so there is one path in and
        // one set of rules about names.
        .dropDestination(for: URL.self) { urls, _ in
            mesh.importFiles(urls.filter(\.isFileURL))
            return true
        } isTargeted: { dropTargeted = $0 }
        .overlay {
            if dropTargeted {
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(Color.accentColor, lineWidth: 3)
                    .padding(6)
                    .allowsHitTesting(false)
            }
        }
        .fileImporter(
            isPresented: $choosingFiles,
            allowedContentTypes: [.item],
            allowsMultipleSelection: true
        ) { result in
            switch result {
            case .success(let urls): mesh.importFiles(urls)
            case .failure(let error): mesh.announce(error.sentenceForUser, .failure)
            }
        }
    }

    private var deleteRequestSummary: String {
        let count = mesh.state.deleteRequests.count
        let requests = count == 1 ? "1 delete" : "\(count) deletes"
        return "\(requests) waiting for devices that are not on the network. They travel with the catalog."
    }
}

private struct ItemRow: View {
    @Environment(Mesh.self) private var mesh
    let item: Item

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(item.name)
                    .fontWeight(.medium)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 12)
                Text(item.size.asFileSize)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .monospacedDigit()
            }

            HStack(spacing: 6) {
                if item.orphaned {
                    Text("No device holds this")
                        .font(.caption)
                        .foregroundStyle(.red)
                } else {
                    ForEach(item.holders) { HolderChip(holder: $0) }
                }
                Spacer(minLength: 0)
                actions
            }

            ForEach(transfers, id: \.key) { entry in
                ProgressView(value: entry.transfer.fraction) {
                    Text(label(for: entry.transfer))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .progressViewStyle(.linear)
            }
        }
        .padding(.vertical, 4)
        // A row is as wide as the row, not as wide as its contents. The phone
        // learned this the hard way and it costs nothing to get right here.
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }

    @ViewBuilder
    private var actions: some View {
        if item.heldHere, let path = item.localPath {
            Button("Open", systemImage: "arrow.up.forward.app") {
                NSWorkspace.shared.open(URL(fileURLWithPath: path))
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .help("Open \(item.name)")

            Button("Show in Finder", systemImage: "folder") {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .help("Show in Finder")

            Menu {
                if sendable.isEmpty {
                    Text("No paired device is on the network")
                } else {
                    ForEach(sendable, id: \.id) { device in
                        Button(device.name) { mesh.share(item, to: [device.id]) }
                    }
                    if sendable.count > 1 {
                        Divider()
                        Button("Every device here") {
                            mesh.share(item, to: sendable.map(\.id))
                        }
                    }
                }
            } label: {
                Label("Send", systemImage: "paperplane")
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .help("Send a copy to another device")
        } else {
            Button("Take a Copy", systemImage: "arrow.down.circle") {
                mesh.pull(item)
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .help("Take a copy onto this Mac")
        }

        Menu {
            if item.heldHere {
                Button("Delete from this Mac", role: .destructive) {
                    mesh.deleteHere(item)
                }
            }
            let elsewhere = item.holders.filter { !$0.isThisDevice }
            if !elsewhere.isEmpty {
                Divider()
                ForEach(elsewhere) { holder in
                    Button("Delete from \(holder.name)", role: .destructive) {
                        mesh.delete(item, from: holder.deviceId)
                    }
                }
            }
        } label: {
            Label("Delete", systemImage: "trash")
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help("Delete a copy")
    }

    /// Paired, on the network, and not already holding this.
    private var sendable: [PairedDevice] {
        let holders = Set(item.holders.map(\.deviceId))
        return mesh.state.reachable.filter { !holders.contains($0.id) }
    }

    private var transfers: [(key: String, transfer: Transfer)] {
        mesh.transfers
            .filter { $0.value.fileId == item.id }
            .map { (key: $0.key, transfer: $0.value) }
            .sorted { $0.key < $1.key }
    }

    private func label(for transfer: Transfer) -> String {
        let blocks = "\(transfer.blocksDone) of \(transfer.blocksTotal) blocks"
        switch transfer.direction {
        case .sending:
            let peer = transfer.peerId.map(nameOf) ?? "another device"
            return "Sending to \(peer) — \(blocks)"
        case .receiving:
            return "Receiving — \(blocks)"
        }
    }

    private func nameOf(_ deviceId: String) -> String {
        mesh.state.paired.first { $0.id == deviceId }?.name ?? shortId(deviceId)
    }
}

/// A name that arrived and could not have it.
///
/// The engine already settled this the safe way - both items kept, the incoming
/// one numbered - and is asking whether it should have been an override
/// instead. Nothing is waiting on the answer, which is why this is a banner
/// rather than a modal.
private struct CollisionBanner: View {
    @Environment(Mesh.self) private var mesh
    let collisions: [PendingCollision]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(collisions, id: \.id) { collision in
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("\(displayName(collision.requestedPath)) was already taken.")
                            .font(.callout)
                        Text("It was kept as \(displayName(collision.currentPath)).")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 12)
                    Button("Keep Both") { mesh.resolve(collision, as: .keepBoth) }
                    Button("Replace the Old One") { mesh.resolve(collision, as: .override) }
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.yellow.opacity(0.12))
    }
}
