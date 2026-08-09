import Foundation

/// What the engine currently believes, shaped for a window rather than for a
/// database.
///
/// The engine hands back a catalog and a separate list of holders, keyed by id,
/// which is the right shape for replication and the wrong one for a list of
/// rows. Joining them - and resolving device ids to the names of devices you
/// have actually paired with - happens once, in `EngineWorker.snapshot()`, so
/// no view ever holds an id it has to look up.
struct MeshState: Sendable {
    /// False until the engine has been constructed and read once.
    var ready = false
    var running = false
    /// Set when the engine could not be created or started at all.
    var fatal: String?
    var thisDevice = ThisDevice()
    var items: [Item] = []
    var trash: [TrashItem] = []
    var visible: [DiscoveredDevice] = []
    var paired: [PairedDevice] = []
    var offers: [PairingOffer] = []
    var collisions: [PendingCollision] = []
    var deleteRequests: [DeleteRequest] = []
    /// Where this device keeps what it holds, according to the engine rather
    /// than to the app's guess at it.
    var syncDirectory = ""

    /// Paired devices that are also on the network right now.
    var reachable: [PairedDevice] {
        let seen = Set(visible.map(\.deviceId))
        return paired.filter { seen.contains($0.id) }
    }

    /// Devices on the network that have never been paired with. The only ones
    /// worth offering a Pair button for.
    var strangers: [DiscoveredDevice] {
        let known = Set(paired.map(\.id))
        return visible.filter { !known.contains($0.deviceId) }
    }
}

struct ThisDevice: Sendable {
    var id = ""
    var name = ""
    var platform = ""

    var shortId: String { LocalCloud.shortId(id) }
}

/// One item in the shared catalog.
///
/// `heldHere` is separate from `holders` on purpose. Whether *this* device has
/// the bytes decides what the row can offer - you cannot share what you do not
/// hold, and pulling what you already have is meaningless - so it is answered
/// once rather than by searching the holder list at every call site.
struct Item: Sendable, Identifiable {
    let id: String
    let name: String
    let size: Int64
    let heldHere: Bool
    let holders: [Holder]
    let modifiedTime: Int64
    /// Where the bytes are on this device, or nil when they are not. Resolved
    /// once, here, rather than by every view that wants to offer an Open button.
    let localPath: String?

    /// Nobody at all holds this, which the catalog can legitimately say.
    var orphaned: Bool { holders.isEmpty }
}

/// A device that holds a copy, named rather than identified.
struct Holder: Sendable, Identifiable {
    let deviceId: String
    let name: String
    let isThisDevice: Bool
    let reachable: Bool

    var id: String { deviceId }
}

struct TrashItem: Sendable, Identifiable {
    let id: String
    let name: String
    let size: Int64
    /// Nil when the engine no longer has a countdown for it.
    let secondsRemaining: Int64?
    let trashedBy: String
}

/// A copy on the move.
///
/// Keyed by item *and* peer, because `shareTo` sends to several devices at once
/// and each reports its own progress - one bar per destination, not one bar
/// that jumps backwards as a second device starts.
struct Transfer: Sendable {
    enum Direction: Sendable { case sending, receiving }

    let fileId: String
    let direction: Direction
    let peerId: String?
    let blocksDone: UInt64
    let blocksTotal: UInt64

    var fraction: Double {
        guard blocksTotal > 0 else { return 0 }
        return min(1, Double(blocksDone) / Double(blocksTotal))
    }

    static func key(_ fileId: String, _ peerId: String?) -> String {
        "\(fileId)@\(peerId ?? "here")"
    }
}

/// Something worth telling the person about, once.
struct Notice: Sendable, Identifiable {
    enum Kind: Sendable { case info, failure }

    let id: Int
    let text: String
    let kind: Kind
}

/// The engine stores a path within the sync folder; a person wants the name.
func displayName(_ path: String) -> String {
    path.split(separator: "/").last.map(String.init) ?? path
}

/// Enough of a device id to tell two apart, when there is no name for it.
func shortId(_ deviceId: String) -> String {
    String(deviceId.prefix(8))
}
