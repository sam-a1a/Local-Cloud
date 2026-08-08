package com.ghazaleh.localcloud.service

import com.ghazaleh.localcloud.engine.MeshState
import com.ghazaleh.localcloud.engine.ThisDevice
import com.ghazaleh.localcloud.engine.Transfer
import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.localcloud.DiscoveredDevice
import uniffi.localcloud.PairedDevice

/**
 * What the notification says, which is the only thing the service says at all.
 *
 * Worth testing rather than eyeballing: this line is the whole of the app's
 * presence while it is closed, it is the thing a person judges "is this doing
 * anything?" by, and every one of its branches is a state that is tedious to
 * reach by hand on a device - six hours of syncing, a peer going quiet, two
 * transfers crossing.
 */
class SyncSummaryTest {

    @Test
    fun `a stopped engine says so rather than claiming a mesh`() {
        assertEquals("Starting…", summarise(mesh(running = false), noTransfers))
    }

    @Test
    fun `with nothing paired it says what is missing`() {
        assertEquals(
            "Waiting to be paired with another device",
            summarise(mesh(), noTransfers),
        )
    }

    @Test
    fun `a paired device that is not here is not counted as here`() {
        val state = mesh(paired = listOf(paired("mac")), visible = emptyList())

        assertEquals("No paired devices on this network", summarise(state, noTransfers))
    }

    @Test
    fun `devices on the network are counted, and counted in English`() {
        val one = mesh(paired = listOf(paired("mac")), visible = listOf(seen("mac")))
        assertEquals("On the mesh with 1 device", summarise(one, noTransfers))

        val two = mesh(
            paired = listOf(paired("mac"), paired("phone")),
            visible = listOf(seen("mac"), seen("phone")),
        )
        assertEquals("On the mesh with 2 devices", summarise(two, noTransfers))
    }

    @Test
    fun `a transfer is more interesting than a device count`() {
        val state = mesh(paired = listOf(paired("mac")), visible = listOf(seen("mac")))

        assertEquals(
            "Sending 1 file",
            summarise(state, transfers(sending = 1)),
        )
        assertEquals(
            "Receiving 2 files",
            summarise(state, transfers(receiving = 2)),
        )
    }

    @Test
    fun `traffic in both directions is reported in both directions`() {
        val state = mesh(paired = listOf(paired("mac")), visible = listOf(seen("mac")))

        assertEquals(
            "Sending 1, receiving 2",
            summarise(state, transfers(sending = 1, receiving = 2)),
        )
    }

    @Test
    fun `a transfer while the engine is down does not claim to be moving`() {
        // The engine going away should not leave the notification describing
        // transfers that cannot be running.
        assertEquals(
            "Starting…",
            summarise(mesh(running = false), transfers(sending = 3)),
        )
    }

    // -- Builders -----------------------------------------------------------

    private val noTransfers = emptyMap<String, Transfer>()

    private fun mesh(
        running: Boolean = true,
        paired: List<PairedDevice> = emptyList(),
        visible: List<DiscoveredDevice> = emptyList(),
    ) = MeshState(
        ready = true,
        running = running,
        thisDevice = ThisDevice(id = "self", name = "This phone", platform = "Android"),
        paired = paired,
        visible = visible,
    )

    private fun paired(id: String) = PairedDevice(
        id = id,
        name = id,
        platform = "macOS",
        certPem = "",
        pairedAt = 0,
        lastSeen = 0,
    )

    private fun seen(id: String) = DiscoveredDevice(
        deviceId = id,
        name = id,
        platform = "macOS",
        url = "https://127.0.0.1:1",
    )

    private fun transfers(sending: Int = 0, receiving: Int = 0): Map<String, Transfer> =
        buildMap {
            repeat(sending) { index ->
                put(
                    "s$index",
                    Transfer("file$index", Transfer.Direction.Sending, "mac", 1, 4),
                )
            }
            repeat(receiving) { index ->
                put(
                    "r$index",
                    Transfer("file$index", Transfer.Direction.Receiving, null, 1, 4),
                )
            }
        }
}
