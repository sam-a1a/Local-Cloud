import SwiftUI

/// The mesh, and how devices join it.
///
/// Three lists, in the order the question comes up: who has asked to pair with
/// this Mac, who is on the network but is nobody yet, and who this Mac already
/// trusts.
struct DevicesScreen: View {
    @Environment(Mesh.self) private var mesh

    /// The code this Mac is showing, and to whom. Set when pairing is started
    /// from this side; the other device types it.
    @State private var showing: (code: String, name: String)?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                if !mesh.state.offers.isEmpty {
                    Panel(title: "Asked to pair with this Mac") {
                        ForEach(mesh.state.offers, id: \.deviceId) { offer in
                            OfferRow(offer: offer)
                        }
                    }
                }

                Panel(
                    title: "On the network",
                    subtitle: mesh.state.strangers.isEmpty ? "nothing else found yet" : nil
                ) {
                    ForEach(mesh.state.strangers, id: \.deviceId) { device in
                        HStack {
                            ReachabilityDot(reachable: true)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(device.name)
                                Text("\(device.platform) · \(shortId(device.deviceId))")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer(minLength: 12)
                            Button("Pair") {
                                Task {
                                    if let code = await mesh.beginPairing(with: device) {
                                        showing = (code, device.name)
                                    }
                                }
                            }
                        }
                        .padding(.vertical, 2)
                    }
                }

                Panel(
                    title: "Paired",
                    subtitle: mesh.state.paired.isEmpty ? "no devices yet" : nil
                ) {
                    ForEach(mesh.state.paired, id: \.id) { device in
                        PairedRow(device: device, reachable: isReachable(device))
                    }
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .sheet(item: Binding(
            get: { showing.map { PairingCode(code: $0.code, name: $0.name) } },
            set: { if $0 == nil { showing = nil } }
        )) { pairing in
            PairingSheet(pairing: pairing) {
                mesh.cancelPairing()
                showing = nil
            }
        }
    }

    private func isReachable(_ device: PairedDevice) -> Bool {
        mesh.state.visible.contains { $0.deviceId == device.id }
    }
}

private struct PairingCode: Identifiable {
    let code: String
    let name: String
    var id: String { code }
}

/// Six digits, large, and nothing else to look at.
///
/// The code is only good for a couple of minutes and only for the device it was
/// started for, so the sheet says whose it is. Closing it cancels, because a
/// pairing nobody is watching is one nobody will finish.
private struct PairingSheet: View {
    let pairing: PairingCode
    let cancel: () -> Void

    var body: some View {
        VStack(spacing: 16) {
            Text("Type this on \(pairing.name)")
                .font(.headline)
            Text(spaced(pairing.code))
                .font(.system(size: 44, weight: .semibold, design: .monospaced))
                .textSelection(.enabled)
            Text("The code expires in a couple of minutes.")
                .font(.caption)
                .foregroundStyle(.secondary)
            Button("Cancel", action: cancel)
                .keyboardShortcut(.cancelAction)
        }
        .padding(28)
        .frame(minWidth: 340)
    }

    /// 123 456 rather than 123456: six digits read off a screen and typed into
    /// another one are easier in two threes.
    private func spaced(_ code: String) -> String {
        guard code.count == 6 else { return code }
        let middle = code.index(code.startIndex, offsetBy: 3)
        return "\(code[code.startIndex..<middle]) \(code[middle...])"
    }
}

/// Another device started pairing and this Mac has to type the code.
private struct OfferRow: View {
    @Environment(Mesh.self) private var mesh
    let offer: PairingOffer
    @State private var code = ""
    @State private var busy = false

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 1) {
                Text(offer.name)
                Text("\(offer.platform) · \(shortId(offer.deviceId))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            TextField("000000", text: $code)
                .textFieldStyle(.roundedBorder)
                .font(.system(.body, design: .monospaced))
                .frame(width: 90)
                .onSubmit(confirm)
            Button("Confirm", action: confirm)
                .disabled(code.count != 6 || busy)
        }
        .padding(.vertical, 2)
    }

    private func confirm() {
        guard code.count == 6, !busy else { return }
        busy = true
        Task {
            if await mesh.confirmPairing(with: offer.deviceId, code: code) {
                code = ""
            }
            busy = false
        }
    }
}

private struct PairedRow: View {
    @Environment(Mesh.self) private var mesh
    let device: PairedDevice
    let reachable: Bool

    var body: some View {
        HStack {
            ReachabilityDot(reachable: reachable)
            VStack(alignment: .leading, spacing: 1) {
                Text(device.name)
                Text(reachable
                     ? "\(device.platform) · \(shortId(device.id))"
                     : "\(device.platform) · not on the network")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            Button("Unpair", role: .destructive) { mesh.unpair(device) }
        }
        .padding(.vertical, 2)
    }
}
