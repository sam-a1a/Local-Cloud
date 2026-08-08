package com.ghazaleh.localcloud.ui.devices

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.ghazaleh.localcloud.engine.MeshState
import com.ghazaleh.localcloud.ui.components.EmptyState
import com.ghazaleh.localcloud.ui.components.SectionHeader
import com.ghazaleh.localcloud.ui.components.StatusDot
import com.ghazaleh.localcloud.ui.icons.LocalCloudIcons
import com.ghazaleh.localcloud.ui.theme.IdentifierStyle
import uniffi.localcloud.DiscoveredDevice
import uniffi.localcloud.PairedDevice
import uniffi.localcloud.PairingOffer

/**
 * The mesh, in the three states a device can be in.
 *
 * Ordered by what needs a person: a device asking to pair is at the top because
 * it is waiting on a decision, paired devices come next because they are the
 * mesh, and devices merely seen on the network are last because seeing one is
 * not yet a relationship.
 */
@Composable
fun DevicesScreen(
    state: MeshState,
    onPair: (DiscoveredDevice) -> Unit,
    onOpenOffer: (PairingOffer) -> Unit,
    onUnpair: (String) -> Unit,
    onRename: () -> Unit,
    contentPadding: PaddingValues,
    modifier: Modifier = Modifier,
) {
    var unpairing by remember { mutableStateOf<PairedDevice?>(null) }

    val pairedIds = state.paired.map { it.id }.toSet()
    val offeringIds = state.offers.map { it.deviceId }.toSet()
    val strangers = state.visible.filter {
        it.deviceId !in pairedIds && it.deviceId !in offeringIds && it.deviceId != state.thisDevice.id
    }
    val onlineIds = state.visible.map { it.deviceId }.toSet()

    LazyColumn(
        modifier = modifier.fillMaxWidth(),
        contentPadding = contentPadding,
    ) {
        item(key = "this-device") {
            ThisDeviceCard(state, onRename)
        }

        if (state.offers.isNotEmpty()) {
            item(key = "offers-header") { SectionHeader("Waiting to pair") }
            items(state.offers, key = { "offer-${it.deviceId}" }) { offer ->
                DeviceCard(
                    name = offer.name,
                    detail = "${offer.platform} · asked to pair with this device",
                    online = true,
                    accent = true,
                    action = {
                        Button(onClick = { onOpenOffer(offer) }) { Text("Enter code") }
                    },
                )
            }
        }

        if (state.paired.isNotEmpty()) {
            item(key = "paired-header") {
                SectionHeader("Paired", trailing = "${state.paired.size}")
            }
            items(state.paired, key = { "paired-${it.id}" }) { device ->
                val online = device.id in onlineIds
                DeviceCard(
                    name = device.name,
                    detail = if (online) {
                        "${device.platform} · on this network"
                    } else {
                        "${device.platform} · not reachable right now"
                    },
                    online = online,
                    identifier = device.id,
                    action = {
                        TextButton(onClick = { unpairing = device }) { Text("Unpair") }
                    },
                )
            }
        }

        if (strangers.isNotEmpty()) {
            item(key = "strangers-header") { SectionHeader("On this network") }
            items(strangers, key = { "seen-${it.deviceId}" }) { device ->
                DeviceCard(
                    name = device.name,
                    detail = "${device.platform} · not paired",
                    online = true,
                    identifier = device.deviceId,
                    action = {
                        Button(onClick = { onPair(device) }) { Text("Pair") }
                    },
                )
            }
        }

        if (state.paired.isEmpty() && strangers.isEmpty() && state.offers.isEmpty()) {
            item(key = "empty") {
                EmptyState(
                    icon = LocalCloudIcons.Devices,
                    title = "No other devices yet",
                    body = if (state.running) {
                        "Open LocalCloud on another device on this Wi-Fi network and it will appear here."
                    } else {
                        "The engine is not running, so nothing is being looked for."
                    },
                )
            }
        }
    }

    val target = unpairing
    if (target != null) {
        AlertDialog(
            onDismissRequest = { unpairing = null },
            title = { Text("Unpair ${target.name}?") },
            text = {
                Text(
                    "The two devices stop trusting each other and stop exchanging catalogs. " +
                        "Files already on this device stay here. Pairing again means another six-digit code."
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        onUnpair(target.id)
                        unpairing = null
                    }
                ) { Text("Unpair") }
            },
            dismissButton = {
                TextButton(onClick = { unpairing = null }) { Text("Keep") }
            },
        )
    }
}

@Composable
private fun ThisDeviceCard(state: MeshState, onRename: () -> Unit) {
    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 12.dp),
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.primaryContainer,
        contentColor = MaterialTheme.colorScheme.onPrimaryContainer,
    ) {
        Column(modifier = Modifier.padding(20.dp)) {
            Text(
                text = "THIS DEVICE",
                style = MaterialTheme.typography.labelMedium,
            )
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.padding(top = 4.dp),
            ) {
                Text(
                    text = state.thisDevice.name.ifBlank { "Unnamed device" },
                    style = MaterialTheme.typography.headlineSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                TextButton(onClick = onRename) { Text("Rename") }
            }
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                modifier = Modifier.padding(top = 8.dp),
            ) {
                StatusDot(on = state.running)
                Text(
                    text = if (state.running) {
                        "Discoverable on this network"
                    } else {
                        "Not running"
                    },
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            Text(
                text = state.thisDevice.id.take(16),
                style = IdentifierStyle,
                modifier = Modifier.padding(top = 8.dp),
            )
        }
    }
}

@Composable
private fun DeviceCard(
    name: String,
    detail: String,
    online: Boolean,
    modifier: Modifier = Modifier,
    identifier: String? = null,
    accent: Boolean = false,
    action: @Composable () -> Unit,
) {
    Card(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 4.dp),
        shape = RoundedCornerShape(16.dp),
        colors = CardDefaults.cardColors(
            containerColor = if (accent) {
                MaterialTheme.colorScheme.tertiaryContainer
            } else {
                MaterialTheme.colorScheme.surfaceContainer
            },
            contentColor = if (accent) {
                MaterialTheme.colorScheme.onTertiaryContainer
            } else {
                MaterialTheme.colorScheme.onSurface
            },
        ),
    ) {
        Row(
            modifier = Modifier.padding(start = 16.dp, end = 8.dp, top = 12.dp, bottom = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    StatusDot(on = online)
                    Text(
                        text = name,
                        style = MaterialTheme.typography.titleMedium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Text(
                    text = detail,
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.padding(top = 2.dp),
                )
                if (identifier != null) {
                    Text(
                        text = identifier.take(16),
                        style = IdentifierStyle,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                }
            }
            action()
        }
    }
}
