package com.ghazaleh.localcloud.engine

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.localcloud.CollisionResolution
import uniffi.localcloud.Engine
import uniffi.localcloud.EngineEvent
import uniffi.localcloud.EventListener
import java.util.concurrent.atomic.AtomicLong

/**
 * Everything the app knows about the mesh, and every way it can change it.
 *
 * Two rules shape this class.
 *
 * **Every engine call blocks.** The API is deliberately synchronous - it
 * returns as soon as a request is understood and finishes the network part in
 * the background - but "as soon as" still means a database write on the calling
 * thread. Nothing here touches the engine off [Dispatchers.IO].
 *
 * **Events arrive on the engine's own thread.** The engine's contract says a
 * listener must not block and must not call back in and wait, so [onEvent] only
 * ever writes to a flow or a conflated channel, and the reading is done by a
 * coroutine that owns it.
 */
class EngineRepository(
    private val host: EngineHost,
    private val scope: CoroutineScope,
) : EventListener {

    private val _state = MutableStateFlow(MeshState())
    val state: StateFlow<MeshState> = _state.asStateFlow()

    private val _transfers = MutableStateFlow<Map<String, Transfer>>(emptyMap())
    val transfers: StateFlow<Map<String, Transfer>> = _transfers.asStateFlow()

    private val _notices = MutableSharedFlow<Notice>(
        extraBufferCapacity = 16,
        onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )
    val notices: SharedFlow<Notice> = _notices.asSharedFlow()

    /**
     * Conflated on purpose. A burst of thirty block-progress events should
     * cause one re-read of the catalog, not thirty.
     */
    private val refreshRequests = Channel<Unit>(Channel.CONFLATED)

    private val noticeIds = AtomicLong()

    @Volatile
    private var polling = false
    private var begun = false

    /**
     * Constructs the engine, registers for events, and starts reading.
     *
     * The listener is set before anything else can start it, which is what the
     * engine's backlog exists to make unnecessary - but arriving late to your
     * own events is still a poor way to run a UI.
     */
    fun begin() {
        if (begun) return
        begun = true

        scope.launch(Dispatchers.IO) {
            try {
                host.setEventListener(this@EngineRepository)
            } catch (t: Throwable) {
                _state.update { it.copy(ready = true, fatal = t.describeForUser()) }
                return@launch
            }
            for (ignored in refreshRequests) {
                try {
                    readEverything()
                } catch (t: Throwable) {
                    _state.update { it.copy(ready = true, fatal = t.describeForUser()) }
                }
            }
        }

        // Discovery and local changes both announce themselves, but a catalog
        // that changed on a peer does not: replication is pull-only, and the
        // engine re-reads its peers every 30 seconds without saying so (§14).
        // Nor does a device going quiet produce an event. So the screen is kept
        // honest by asking, cheaply, on a timer - and the timer only runs while
        // the engine does.
        scope.launch {
            while (isActive) {
                if (polling) refreshRequests.trySend(Unit)
                delay(POLL_INTERVAL_MS)
            }
        }
    }

    /** Foreground: discovery, the server, and replication all come up. */
    suspend fun resume() {
        withContext(Dispatchers.IO) {
            try {
                host.start()
            } catch (t: Throwable) {
                _state.update { it.copy(ready = true, fatal = t.describeForUser()) }
                return@withContext
            }
            polling = true
            refreshRequests.trySend(Unit)
        }
    }

    /** Background: stop, and give the multicast lock back. */
    suspend fun pause() {
        withContext(Dispatchers.IO) {
            polling = false
            runCatching { host.stop() }
            _transfers.value = emptyMap()
            refreshRequests.trySend(Unit)
        }
    }

    // -- Reading ------------------------------------------------------------

    private fun readEverything() {
        val engine = host.engine

        val thisId = engine.deviceId()
        val catalog = engine.catalog()
        val paired = engine.pairedDevices()
        val visible = engine.visibleDevices()
        val offers = engine.pairingOffers()
        val trashed = engine.trashedFiles()
        val collisions = engine.pendingCollisions()
        val deleteRequests = engine.pendingDeleteRequests()

        val names = HashMap<String, String>()
        names[thisId] = engine.deviceName()
        paired.forEach { names[it.id] = it.name }
        visible.forEach { names.getOrPut(it.deviceId) { it.name } }

        val reachableIds = visible.mapTo(HashSet()) { it.deviceId }.apply { add(thisId) }
        val holdersByFile = catalog.holders.groupBy { it.fileId }

        fun holdersFor(fileId: String): List<Holder> =
            holdersByFile[fileId].orEmpty()
                .map { holder ->
                    Holder(
                        deviceId = holder.deviceId,
                        name = names[holder.deviceId] ?: shortId(holder.deviceId),
                        isThisDevice = holder.deviceId == thisId,
                        reachable = holder.deviceId in reachableIds,
                    )
                }
                // This device first: the answer to "do I have this?" should be
                // in the same place on every row.
                .sortedWith(compareByDescending<Holder> { it.isThisDevice }.thenBy { it.name })

        val items = catalog.items
            .filter { it.trashedAt == 0L }
            .map { meta ->
                val holders = holdersFor(meta.id)
                Item(
                    id = meta.id,
                    name = displayName(meta.path),
                    size = meta.size,
                    heldHere = holders.any { it.isThisDevice },
                    holders = holders,
                    modifiedTime = meta.modifiedTime,
                )
            }
            .sortedByDescending { it.modifiedTime }

        val trash = trashed.map { meta ->
            TrashItem(
                id = meta.id,
                name = displayName(meta.path),
                size = meta.size,
                secondsRemaining = engine.trashSecondsRemaining(meta.id),
                trashedBy = names[meta.trashedBy] ?: shortId(meta.trashedBy),
            )
        }

        _state.value = MeshState(
            ready = true,
            running = engine.isRunning(),
            fatal = null,
            thisDevice = ThisDevice(
                id = thisId,
                name = engine.deviceName(),
                platform = engine.devicePlatform(),
            ),
            items = items,
            trash = trash,
            visible = visible,
            paired = paired,
            offers = offers,
            collisions = collisions,
            deleteRequests = deleteRequests,
        )
    }

    // -- Doing --------------------------------------------------------------

    suspend fun importFile(sourcePath: String, name: String): Boolean =
        attempt { it.importFile(sourcePath, name) } != null

    suspend fun share(fileId: String, deviceIds: List<String>) {
        attempt { it.shareTo(fileId, deviceIds) }
    }

    suspend fun pull(fileId: String) {
        attempt { it.pullCopy(fileId) }
    }

    suspend fun deleteHere(fileId: String) {
        val outcome = attempt { it.deleteLocalCopy(fileId) } ?: return
        announce(
            if (outcome.trashed) {
                "That was the last copy. It is in the trash for 30 days."
            } else {
                "Deleted here. ${outcome.remainingCopies} ${if (outcome.remainingCopies == 1L) "copy" else "copies"} left elsewhere."
            }
        )
    }

    suspend fun deleteFrom(fileId: String, deviceId: String) {
        attempt { it.deleteCopy(fileId, deviceId) }
    }

    suspend fun beginPairing(deviceId: String): String? =
        attempt { it.startPairing(listOf(deviceId)) }

    suspend fun confirmPairing(deviceId: String, code: String): Boolean =
        attempt { it.confirmPairing(deviceId, code) } != null

    suspend fun cancelPairing() {
        attempt { it.cancelPairing() }
    }

    suspend fun unpair(deviceId: String) {
        attempt { it.unpair(deviceId) }
    }

    suspend fun rename(name: String): Boolean =
        attempt { it.setDeviceName(name) } != null

    suspend fun restore(fileId: String) {
        attempt { it.restoreFile(fileId) }
    }

    suspend fun destroy(fileId: String) {
        attempt { it.deletePermanently(fileId) }
    }

    suspend fun resolveCollision(collisionId: String, resolution: CollisionResolution) {
        attempt { it.resolveCollision(collisionId, resolution) }
    }

    /**
     * One place where an engine call is made, fails, and is reported.
     *
     * Returning null rather than throwing because every caller is a tap: there
     * is nothing to recover, only something to say. The refresh afterwards is
     * unconditional - a call that failed part way through still may have
     * changed something.
     */
    private suspend fun <T> attempt(block: (Engine) -> T): T? = withContext(Dispatchers.IO) {
        val result = try {
            block(host.engine)
        } catch (t: Throwable) {
            announce(t.describeForUser(), Notice.Kind.Failure)
            null
        }
        refreshRequests.trySend(Unit)
        result
    }

    // -- Events -------------------------------------------------------------

    /**
     * Called by the engine, on the engine's dispatch thread.
     *
     * Nothing here blocks and nothing here calls back into the engine. The most
     * it does is put a value in a flow and ask, without waiting, for a refresh.
     */
    override fun onEvent(event: EngineEvent) {
        when (event) {
            is EngineEvent.SendProgress -> putTransfer(
                Transfer(
                    fileId = event.fileId,
                    direction = Transfer.Direction.Sending,
                    peerId = event.deviceId,
                    blocksDone = event.blocksDone.toLong(),
                    blocksTotal = event.blocksTotal.toLong(),
                )
            )

            is EngineEvent.ReceiveProgress -> putTransfer(
                Transfer(
                    fileId = event.fileId,
                    direction = Transfer.Direction.Receiving,
                    peerId = null,
                    blocksDone = event.blocksDone.toLong(),
                    blocksTotal = event.blocksTotal.toLong(),
                )
            )

            is EngineEvent.FileSent -> {
                clearTransfer(event.fileId, event.deviceId)
                announce("${displayName(event.path)} reached ${nameOf(event.deviceId)}.")
            }

            is EngineEvent.ShareFailed -> {
                clearTransfer(event.fileId, event.deviceId)
                announce(
                    "${displayName(event.path)} did not reach ${nameOf(event.deviceId)}: ${event.reason}",
                    Notice.Kind.Failure,
                )
            }

            is EngineEvent.FileDownloaded -> {
                clearTransfer(event.fileId, null)
                announce("${displayName(event.path)} is now on this device.")
            }

            is EngineEvent.PullFailed -> {
                clearTransfer(event.fileId, null)
                announce("Could not take a copy: ${event.reason}", Notice.Kind.Failure)
            }

            is EngineEvent.DevicePaired -> announce("Paired with ${event.name}.")
            is EngineEvent.PairingFailed ->
                announce("Pairing failed: ${event.reason}", Notice.Kind.Failure)

            is EngineEvent.NameCollision -> announce(
                "${displayName(event.requestedPath)} was already taken, so it arrived as ${displayName(event.keptAs)}."
            )

            is EngineEvent.DeleteRequestDeferred -> announce(
                "${nameOf(event.deviceId)} could not be reached, so the delete will travel with the catalog."
            )

            is EngineEvent.EngineFailed ->
                announce(event.reason, Notice.Kind.Failure)

            // Everything else is a change to state the refresh below will pick
            // up - there is nothing to say about it that the screen will not
            // show on its own.
            else -> Unit
        }
        refreshRequests.trySend(Unit)
    }

    private fun putTransfer(transfer: Transfer) {
        _transfers.update { it + (Transfer.key(transfer.fileId, transfer.peerId) to transfer) }
    }

    private fun clearTransfer(fileId: String, peerId: String?) {
        _transfers.update { it - Transfer.key(fileId, peerId) }
    }

    private fun announce(text: String, kind: Notice.Kind = Notice.Kind.Info) {
        _notices.tryEmit(Notice(text, kind, noticeIds.incrementAndGet()))
    }

    private fun nameOf(deviceId: String): String =
        _state.value.paired.firstOrNull { it.id == deviceId }?.name
            ?: _state.value.visible.firstOrNull { it.deviceId == deviceId }?.name
            ?: shortId(deviceId)

    private companion object {
        const val POLL_INTERVAL_MS = 2_000L
    }
}

/** The engine stores a path within the sync folder; a person wants the name. */
internal fun displayName(path: String): String = path.substringAfterLast('/')

/** Enough of a device id to tell two apart, when there is no name for it. */
internal fun shortId(deviceId: String): String = deviceId.take(8)
