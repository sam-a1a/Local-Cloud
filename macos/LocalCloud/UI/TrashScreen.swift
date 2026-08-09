import SwiftUI

/// The thirty days between the last copy of an item being deleted and its bytes
/// being released.
///
/// An item reaches this only when nobody holds it any more - deleting one copy
/// of three just removes a holder - which is why the countdown is the most
/// important thing on the row.
struct TrashScreen: View {
    @Environment(Mesh.self) private var mesh

    var body: some View {
        if mesh.state.trash.isEmpty {
            ContentUnavailableView {
                Label("Nothing in the trash", systemImage: "trash")
            } description: {
                Text("An item comes here when its last copy is deleted, and stays for 30 days.")
            }
        } else {
            List(mesh.state.trash) { item in
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(item.name)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Text(remaining(item))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 12)
                    Button("Restore") { mesh.restore(item) }
                    Button("Delete Now", role: .destructive) { mesh.destroy(item) }
                }
                .padding(.vertical, 2)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .listStyle(.inset)
        }
    }

    private func remaining(_ item: TrashItem) -> String {
        let size = item.size.asFileSize
        guard let seconds = item.secondsRemaining else {
            return "\(size) · deleted by \(item.trashedBy)"
        }
        return "\(size) · \(timeRemaining(seconds)) · deleted by \(item.trashedBy)"
    }
}
