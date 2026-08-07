package com.ghazaleh.localcloud.engine

import uniffi.localcloud.DeleteRequest
import uniffi.localcloud.DiscoveredDevice
import uniffi.localcloud.PairedDevice
import uniffi.localcloud.PairingOffer
import uniffi.localcloud.PendingCollision

/**
 * What the engine currently believes, shaped for a screen rather than for a
 * database.
 *
 * The engine hands back a catalog and a separate list of holders, keyed by id,
 * which is the right shape for replication and the wrong one for a list of
 * rows. Joining them - and resolving device ids to the names of devices you
 * have actually paired with - happens once, here, so no composable ever holds
 * an id it has to look up.
 */
data class MeshState(
    /** False until the engine has been constructed and read once. */
    val ready: Boolean = false,
    val running: Boolean = false,
    /** Set when the engine could not be created or started at all. */
    val fatal: String? = null,
    val thisDevice: ThisDevice = ThisDevice(),
    val items: List<Item> = emptyList(),
    val trash: List<TrashItem> = emptyList(),
    val visible: List<DiscoveredDevice> = emptyList(),
    val paired: List<PairedDevice> = emptyList(),
    val offers: List<PairingOffer> = emptyList(),
    val collisions: List<PendingCollision> = emptyList(),
    val deleteRequests: List<DeleteRequest> = emptyList(),
) {
    /** Paired devices that are also on the network right now. */
    val reachable: List<PairedDevice>
        get() {
            val seen = visible.map { it.deviceId }.toSet()
            return paired.filter { it.id in seen }
        }
}

data class ThisDevice(
    val id: String = "",
    val name: String = "",
    val platform: String = "",
)

/**
 * One item in the shared catalog.
 *
 * `heldHere` is separate from `holders` on purpose. Whether *this* device has
 * the bytes decides what the row can offer - you cannot share what you do not
 * hold, and pulling what you already have is meaningless - so it is answered
 * once rather than by searching the holder list at every call site.
 */
data class Item(
    val id: String,
    val name: String,
    val size: Long,
    val heldHere: Boolean,
    val holders: List<Holder>,
    val modifiedTime: Long,
) {
    val holderCount: Int get() = holders.size

    /** Nobody at all holds this, which the catalog can legitimately say. */
    val orphaned: Boolean get() = holders.isEmpty()
}

/**
 * A device that holds a copy, named rather than identified.
 *
 * `isThisDevice` drives the colour: cool for here, warm for elsewhere. See the
 * note in Color.kt.
 */
data class Holder(
    val deviceId: String,
    val name: String,
    val isThisDevice: Boolean,
    val reachable: Boolean,
)

data class TrashItem(
    val id: String,
    val name: String,
    val size: Long,
    /** Null when the engine no longer has a countdown for it. */
    val secondsRemaining: Long?,
    val trashedBy: String,
)

/**
 * A copy on the move.
 *
 * Keyed by item *and* peer, because `share_to` sends to several devices at
 * once and each reports its own progress - one bar per destination, not one bar
 * that jumps backwards as a second device starts.
 */
data class Transfer(
    val fileId: String,
    val direction: Direction,
    val peerId: String?,
    val blocksDone: Long,
    val blocksTotal: Long,
) {
    val fraction: Float
        get() = if (blocksTotal <= 0L) 0f else (blocksDone.toFloat() / blocksTotal.toFloat()).coerceIn(0f, 1f)

    enum class Direction { Sending, Receiving }

    companion object {
        fun key(fileId: String, peerId: String?): String = "$fileId@${peerId ?: "here"}"
    }
}

/** Something worth telling the person about, once. */
data class Notice(
    val text: String,
    val kind: Kind,
    val id: Long,
) {
    enum class Kind { Info, Failure }
}
