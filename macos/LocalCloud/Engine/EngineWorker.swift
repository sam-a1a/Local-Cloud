import Foundation

/// The one engine this process has, and the only thing that ever calls it.
///
/// Every engine call blocks. The API is deliberately synchronous - it returns
/// as soon as a request is understood and finishes the network part in the
/// background - but "as soon as" still means a database write on the calling
/// thread, and on a Mac that thread would otherwise be the one drawing the
/// window. An actor answers both halves of that: it is never the main thread,
/// and it serialises, so a start and a stop can never interleave and leave the
/// engine half-started.
///
/// The methods are spelled out one by one rather than hidden behind a generic
/// `perform { $0.something() }`. It is more lines and a much shorter list of
/// things the rest of the app is able to do to the engine.
///
/// This is why the target compiles with `SWIFT_DEFAULT_ACTOR_ISOLATION` set to
/// `nonisolated` rather than Xcode's new `MainActor` default. That default
/// applies to every file in the module, and the generated bindings are one of
/// them - so the compiler would decide that `Engine.start()` belongs on the
/// main actor and then warn about the one place that correctly refuses to put
/// it there. An app that believed it would call a blocking engine from the
/// thread drawing the window. The app's own types say `@MainActor` where they
/// mean it instead.
actor EngineWorker {

    private let engine: Engine

    init() throws {
        engine = try EngineHost.make()
    }

    func setEventListener(_ listener: EventListener) {
        engine.setEventListener(listener: listener)
    }

    func start() throws {
        try engine.start()
    }

    func stop() {
        engine.stop()
    }

    // -- Reading --------------------------------------------------------------

    /// Everything a window needs, read in one pass.
    ///
    /// One pass because these have to agree with each other: a holder list read
    /// a second after the catalog can name an item the catalog no longer has,
    /// and the row that results is a bug nobody can reproduce.
    func snapshot() -> MeshState {
        let thisId = engine.deviceId()
        let catalog = engine.catalog()
        let paired = engine.pairedDevices()
        let visible = engine.visibleDevices()
        let syncDir = engine.syncDir()

        var names: [String: String] = [thisId: engine.deviceName()]
        for device in paired { names[device.id] = device.name }
        for device in visible where names[device.deviceId] == nil {
            names[device.deviceId] = device.name
        }

        var reachableIds = Set(visible.map(\.deviceId))
        reachableIds.insert(thisId)

        let holdersByFile = Dictionary(grouping: catalog.holders, by: \.fileId)

        func holders(for fileId: String) -> [Holder] {
            (holdersByFile[fileId] ?? [])
                .map { holder in
                    Holder(
                        deviceId: holder.deviceId,
                        name: names[holder.deviceId] ?? shortId(holder.deviceId),
                        isThisDevice: holder.deviceId == thisId,
                        reachable: reachableIds.contains(holder.deviceId)
                    )
                }
                // This device first: the answer to "do I have this?" should be
                // in the same place on every row.
                .sorted { left, right in
                    left.isThisDevice != right.isThisDevice
                        ? left.isThisDevice
                        : left.name.localizedStandardCompare(right.name) == .orderedAscending
                }
        }

        let items = catalog.items
            .filter { $0.trashedAt == 0 }
            .map { meta -> Item in
                let holders = holders(for: meta.id)
                let heldHere = holders.contains(where: \.isThisDevice)
                return Item(
                    id: meta.id,
                    name: displayName(meta.path),
                    size: meta.size,
                    heldHere: heldHere,
                    holders: holders,
                    modifiedTime: meta.modifiedTime,
                    localPath: heldHere ? syncDir + "/" + meta.path : nil
                )
            }
            .sorted { $0.modifiedTime > $1.modifiedTime }

        let trash = engine.trashedFiles().map { meta in
            TrashItem(
                id: meta.id,
                name: displayName(meta.path),
                size: meta.size,
                secondsRemaining: engine.trashSecondsRemaining(fileId: meta.id),
                trashedBy: names[meta.trashedBy] ?? shortId(meta.trashedBy)
            )
        }

        return MeshState(
            ready: true,
            running: engine.isRunning(),
            fatal: nil,
            thisDevice: ThisDevice(
                id: thisId,
                name: engine.deviceName(),
                platform: engine.devicePlatform()
            ),
            items: items,
            trash: trash,
            visible: visible,
            paired: paired,
            offers: engine.pairingOffers(),
            collisions: engine.pendingCollisions(),
            deleteRequests: engine.pendingDeleteRequests(),
            syncDirectory: syncDir
        )
    }

    // -- Doing ----------------------------------------------------------------

    func importFile(at path: String, named name: String) throws {
        _ = try engine.importFile(sourcePath: path, name: name)
    }

    func share(_ fileId: String, to deviceIds: [String]) throws {
        try engine.shareTo(fileId: fileId, targetDeviceIds: deviceIds)
    }

    func pull(_ fileId: String) throws {
        try engine.pullCopy(fileId: fileId)
    }

    func deleteHere(_ fileId: String) throws -> DeleteOutcome {
        try engine.deleteLocalCopy(fileId: fileId)
    }

    func delete(_ fileId: String, from deviceId: String) throws {
        try engine.deleteCopy(fileId: fileId, deviceId: deviceId)
    }

    func beginPairing(with deviceId: String) throws -> String {
        try engine.startPairing(targetDeviceIds: [deviceId])
    }

    func confirmPairing(with deviceId: String, code: String) throws {
        try engine.confirmPairing(deviceId: deviceId, code: code)
    }

    func cancelPairing() {
        engine.cancelPairing()
    }

    func unpair(_ deviceId: String) throws {
        try engine.unpair(deviceId: deviceId)
    }

    func rename(to name: String) throws {
        try engine.setDeviceName(name: name)
    }

    func restore(_ fileId: String) throws {
        try engine.restoreFile(fileId: fileId)
    }

    func destroy(_ fileId: String) throws {
        try engine.deletePermanently(fileId: fileId)
    }

    func resolve(_ collisionId: String, as resolution: CollisionResolution) throws {
        try engine.resolveCollision(collisionId: collisionId, resolution: resolution)
    }
}
