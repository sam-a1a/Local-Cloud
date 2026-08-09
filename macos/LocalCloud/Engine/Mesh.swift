import Foundation
import Observation

/// Everything the app knows about the mesh, and every way it can change it.
///
/// Lives on the main actor and never calls the engine directly: every call goes
/// through [EngineWorker], which is neither the main thread nor two threads at
/// once. What comes back is a whole [MeshState], replaced in one assignment, so
/// a view never sees half of an update.
///
/// Isolated to the main actor by hand, not by the project default. The default
/// would isolate the generated bindings too, and the engine's contract is the
/// opposite of that - see the note in `EngineWorker`.
@Observable
@MainActor
final class Mesh {

    private(set) var state = MeshState()
    private(set) var transfers: [String: Transfer] = [:]
    private(set) var notice: Notice?

    @ObservationIgnored private var worker: EngineWorker?
    @ObservationIgnored private var begun = false
    @ObservationIgnored private var tasks: [Task<Void, Never>] = []
    @ObservationIgnored private var noticeCount = 0

    /// Events, as they leave the engine's own thread.
    ///
    /// The engine's contract is that a listener must not block and must not
    /// call back in and wait. Yielding to a stream does neither. The buffer is
    /// the same 256 the engine keeps on its side, and for the same reason: a
    /// backlog that is capped can say it overflowed, where one that grows just
    /// grows.
    @ObservationIgnored private let events: AsyncStream<EngineEvent>
    @ObservationIgnored private let deliver: @Sendable (EngineEvent) -> Void

    /// Conflated on purpose. A burst of thirty block-progress events should
    /// cause one re-read of the catalog, not thirty.
    @ObservationIgnored private let refreshRequests: AsyncStream<Void>
    @ObservationIgnored private let requestRefresh: @Sendable () -> Void

    init() {
        let (events, eventsIn) = AsyncStream<EngineEvent>.makeStream(
            bufferingPolicy: .bufferingNewest(256)
        )
        self.events = events
        deliver = { eventsIn.yield($0) }

        let (refreshes, refreshesIn) = AsyncStream<Void>.makeStream(
            bufferingPolicy: .bufferingNewest(1)
        )
        refreshRequests = refreshes
        requestRefresh = { refreshesIn.yield(()) }
    }

    // -- Lifecycle ------------------------------------------------------------

    /// Constructs the engine, registers for events, and starts it.
    ///
    /// The listener is registered before the engine is started, which is what
    /// the engine's own backlog exists to make unnecessary - but arriving late
    /// to your own events is still a poor way to run a window.
    func begin() {
        guard !begun else { return }
        begun = true

        Task { [self] in
            let worker: EngineWorker
            do {
                worker = try EngineWorker()
            } catch {
                state.ready = true
                state.fatal = error.sentenceForUser
                return
            }
            self.worker = worker

            let bridge = EventBridge(deliver)
            await worker.setEventListener(bridge)

            tasks.append(Task { [self] in
                for await event in events { handle(event) }
            })
            tasks.append(Task { [self] in
                for await _ in refreshRequests { await refresh() }
            })

            // Discovery and local changes both announce themselves, but a
            // catalog that changed on a peer does not: replication is pull-only,
            // and the engine re-reads its peers every 30 seconds without saying
            // so. Nor does a device going quiet produce an event. So the window
            // is kept honest by asking, cheaply, on a timer.
            tasks.append(Task { [self] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(2))
                    if state.running { requestRefresh() }
                }
            })

            do {
                try await worker.start()
            } catch {
                state.ready = true
                state.fatal = error.sentenceForUser
            }
            requestRefresh()
        }
    }

    /// Stops the engine, on the way out.
    ///
    /// Awaited rather than fired off, because the process is about to end and a
    /// mDNS goodbye that never got sent leaves this device advertised on the
    /// network until every peer times it out.
    func end() async {
        for task in tasks { task.cancel() }
        tasks = []
        await worker?.stop()
    }

    // -- Reading --------------------------------------------------------------

    private func refresh() async {
        guard let worker else { return }
        state = await worker.snapshot()
    }

    // -- Doing ----------------------------------------------------------------

    func importFiles(_ urls: [URL]) {
        Task { [self] in
            for url in urls {
                // A file chosen in an open panel, or dropped on the window, is
                // outside the sandbox until this is called. The engine reads it
                // with plain POSIX calls and has no idea any of this is going
                // on, which is the point: the app owns the sandbox, the engine
                // owns the file.
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }

                await attempt {
                    try await self.worker?.importFile(
                        at: url.path(percentEncoded: false),
                        named: url.lastPathComponent
                    )
                }
            }
        }
    }

    func share(_ item: Item, to deviceIds: [String]) {
        act { try await $0.share(item.id, to: deviceIds) }
    }

    func pull(_ item: Item) {
        act { try await $0.pull(item.id) }
    }

    func deleteHere(_ item: Item) {
        Task { [self] in
            guard let worker else { return }
            let outcome = await attempt { try await worker.deleteHere(item.id) }
            guard let outcome else { return }
            announce(
                outcome.trashed
                    ? "That was the last copy. It is in the trash for 30 days."
                    : "Deleted here. \(copies(outcome.remainingCopies)) left elsewhere."
            )
        }
    }

    func delete(_ item: Item, from deviceId: String) {
        act { try await $0.delete(item.id, from: deviceId) }
    }

    func beginPairing(with device: DiscoveredDevice) async -> String? {
        guard let worker else { return nil }
        return await attempt { try await worker.beginPairing(with: device.deviceId) }
    }

    func confirmPairing(with deviceId: String, code: String) async -> Bool {
        guard let worker else { return false }
        return await attempt { try await worker.confirmPairing(with: deviceId, code: code) } != nil
    }

    func cancelPairing() {
        act { await $0.cancelPairing() }
    }

    func unpair(_ device: PairedDevice) {
        act { try await $0.unpair(device.id) }
    }

    func rename(to name: String) {
        act { try await $0.rename(to: name) }
    }

    func restore(_ item: TrashItem) {
        act { try await $0.restore(item.id) }
    }

    func destroy(_ item: TrashItem) {
        act { try await $0.destroy(item.id) }
    }

    func resolve(_ collision: PendingCollision, as resolution: CollisionResolution) {
        act { try await $0.resolve(collision.id, as: resolution) }
    }

    /// One place where an engine call is made, fails, and is reported.
    ///
    /// Returning nil rather than throwing, because every caller is a click:
    /// there is nothing to recover, only something to say. The refresh
    /// afterwards is unconditional - a call that failed part way through may
    /// still have changed something.
    @discardableResult
    private func attempt<T>(_ body: () async throws -> T) async -> T? {
        defer { requestRefresh() }
        do {
            return try await body()
        } catch {
            announce(error.sentenceForUser, .failure)
            return nil
        }
    }

    /// The common shape: do one thing to the engine, say so if it refuses.
    private func act(_ body: @escaping (EngineWorker) async throws -> Void) {
        Task { [self] in
            guard let worker else { return }
            await attempt { try await body(worker) }
        }
    }

    // -- Events ---------------------------------------------------------------

    private func handle(_ event: EngineEvent) {
        switch event {
        case let .sendProgress(fileId, deviceId, done, total):
            put(Transfer(
                fileId: fileId, direction: .sending, peerId: deviceId,
                blocksDone: done, blocksTotal: total
            ))

        case let .receiveProgress(fileId, done, total):
            put(Transfer(
                fileId: fileId, direction: .receiving, peerId: nil,
                blocksDone: done, blocksTotal: total
            ))

        case let .fileSent(fileId, path, deviceId):
            clear(fileId, deviceId)
            announce("\(displayName(path)) reached \(nameOf(deviceId)).")

        case let .shareFailed(fileId, path, deviceId, reason):
            clear(fileId, deviceId)
            announce(
                "\(displayName(path)) did not reach \(nameOf(deviceId)): \(reason)",
                .failure
            )

        case let .fileDownloaded(fileId, path):
            clear(fileId, nil)
            announce("\(displayName(path)) is now on this device.")

        case let .pullFailed(fileId, reason):
            clear(fileId, nil)
            announce("Could not take a copy: \(reason)", .failure)

        case let .devicePaired(_, name):
            announce("Paired with \(name).")

        case let .pairingFailed(_, reason):
            announce("Pairing failed: \(reason)", .failure)

        case let .nameCollision(requestedPath, keptAs):
            announce(
                "\(displayName(requestedPath)) was already taken, so it arrived as \(displayName(keptAs))."
            )

        case let .deleteRequestDeferred(_, deviceId, _):
            announce(
                "\(nameOf(deviceId)) could not be reached, so the delete will travel with the catalog."
            )

        case let .engineFailed(reason):
            announce(reason, .failure)

        // Everything else is a change to state the refresh below picks up.
        // There is nothing to say about it that the window will not show.
        default:
            break
        }
        requestRefresh()
    }

    private func put(_ transfer: Transfer) {
        transfers[Transfer.key(transfer.fileId, transfer.peerId)] = transfer
    }

    private func clear(_ fileId: String, _ peerId: String?) {
        transfers[Transfer.key(fileId, peerId)] = nil
    }

    private func nameOf(_ deviceId: String) -> String {
        state.paired.first { $0.id == deviceId }?.name
            ?? state.visible.first { $0.deviceId == deviceId }?.name
            ?? shortId(deviceId)
    }

    // -- Saying ---------------------------------------------------------------

    func announce(_ text: String, _ kind: Notice.Kind = .info) {
        noticeCount += 1
        let notice = Notice(id: noticeCount, text: text, kind: kind)
        self.notice = notice

        // Cleared by id rather than unconditionally, so a second notice arriving
        // in the meantime is not wiped by the first one's timer.
        Task { [self] in
            try? await Task.sleep(for: .seconds(8))
            if self.notice?.id == notice.id { self.notice = nil }
        }
    }

    func dismissNotice() {
        notice = nil
    }

    private func copies(_ count: Int64) -> String {
        count == 1 ? "1 copy" : "\(count) copies"
    }
}

/// Carries events from the engine's thread to the stream the app reads.
///
/// Its own type, because the engine calls it from a thread of its own and knows
/// nothing about actors. It holds a closure and no state, so there is nothing
/// here for two threads to disagree about.
private final class EventBridge: EventListener {
    private let deliver: @Sendable (EngineEvent) -> Void

    init(_ deliver: @escaping @Sendable (EngineEvent) -> Void) {
        self.deliver = deliver
    }

    func onEvent(event: EngineEvent) {
        deliver(event)
    }
}
