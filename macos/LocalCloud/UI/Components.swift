import SwiftUI

/// A device that holds a copy, drawn as a chip.
///
/// Cool for here, warm for elsewhere, faded when the device is not on the
/// network - so "where is this?" is answered by colour before it is answered by
/// reading. The same three states the phone uses, for the same reason.
struct HolderChip: View {
    let holder: Holder

    var body: some View {
        Text(holder.isThisDevice ? "This Mac" : holder.name)
            .font(.caption)
            .padding(.horizontal, 7)
            .padding(.vertical, 2)
            .background(background, in: Capsule())
            .foregroundStyle(holder.reachable ? .primary : .secondary)
            .help(holder.reachable ? holder.name : "\(holder.name) — not on the network right now")
    }

    private var background: some ShapeStyle {
        if holder.isThisDevice {
            return AnyShapeStyle(Color.accentColor.opacity(0.18))
        }
        return AnyShapeStyle(Color.orange.opacity(holder.reachable ? 0.18 : 0.08))
    }
}

/// Present, or not, at a glance.
struct ReachabilityDot: View {
    let reachable: Bool

    var body: some View {
        Circle()
            .fill(reachable ? Color.green : Color.secondary.opacity(0.35))
            .frame(width: 8, height: 8)
    }
}

/// What the engine is saying, in the one place it always says it.
struct NoticeBar: View {
    let notice: Notice
    let dismiss: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: notice.kind == .failure
                  ? "exclamationmark.triangle.fill"
                  : "info.circle.fill")
                .foregroundStyle(notice.kind == .failure ? .red : .secondary)
            Text(notice.text)
                .font(.callout)
                .textSelection(.enabled)
            Spacer(minLength: 8)
            Button("Dismiss", systemImage: "xmark", action: dismiss)
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.regularMaterial)
        .overlay(alignment: .top) { Divider() }
    }
}

/// A heading and what sits under it. Not called Section, because SwiftUI has
/// one of those and a shadowed name is a confusing error two files later.
struct Panel<Content: View>: View {
    let title: String
    var subtitle: String?
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                Text(title).font(.headline)
                if let subtitle {
                    Text(subtitle).font(.caption).foregroundStyle(.secondary)
                }
            }
            content
        }
    }
}

extension Int64 {
    /// "4.2 MB", the way the rest of the Mac writes it.
    var asFileSize: String {
        formatted(.byteCount(style: .file))
    }
}

/// How long a trashed item has left, at the resolution a person cares about.
///
/// Days for most of the thirty, then hours, then minutes. Nobody restoring a
/// file wants to be told it has 2,591,999 seconds.
func timeRemaining(_ seconds: Int64) -> String {
    if seconds <= 0 { return "gone shortly" }
    if seconds >= 86_400 {
        let days = seconds / 86_400
        return days == 1 ? "1 day left" : "\(days) days left"
    }
    if seconds >= 3_600 {
        let hours = seconds / 3_600
        return hours == 1 ? "1 hour left" : "\(hours) hours left"
    }
    let minutes = max(1, seconds / 60)
    return minutes == 1 ? "1 minute left" : "\(minutes) minutes left"
}
