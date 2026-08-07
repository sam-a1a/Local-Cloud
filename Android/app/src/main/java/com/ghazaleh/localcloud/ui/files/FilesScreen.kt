package com.ghazaleh.localcloud.ui.files

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.ghazaleh.localcloud.engine.Item
import com.ghazaleh.localcloud.engine.MeshState
import com.ghazaleh.localcloud.engine.Transfer
import com.ghazaleh.localcloud.ui.components.Banner
import com.ghazaleh.localcloud.ui.components.EmptyState
import com.ghazaleh.localcloud.ui.components.HolderChips
import com.ghazaleh.localcloud.ui.components.SectionHeader
import com.ghazaleh.localcloud.ui.components.rememberFormattedSize
import com.ghazaleh.localcloud.ui.icons.LocalCloudIcons
import uniffi.localcloud.PendingCollision

/**
 * The shared catalog: every item any paired device has, and where each one is.
 *
 * A row is closed by default and says only what it is and who has it. Opening
 * one turns it into the model the engine actually keeps - a list of copies, one
 * per device, each of which can be deleted on its own - because that is the
 * idea this app exists to make usable, and hiding it behind an overflow menu
 * would leave the app looking like a folder that syncs.
 */
@Composable
fun FilesScreen(
    state: MeshState,
    transfers: Map<String, Transfer>,
    onShare: (Item) -> Unit,
    onPull: (String) -> Unit,
    onDeleteHere: (String) -> Unit,
    onDeleteFrom: (fileId: String, deviceId: String) -> Unit,
    onOpenCollision: (PendingCollision) -> Unit,
    contentPadding: PaddingValues,
    modifier: Modifier = Modifier,
) {
    var expandedId by rememberSaveable { mutableStateOf<String?>(null) }
    val canSend = state.reachable.isNotEmpty()

    LazyColumn(
        modifier = modifier.fillMaxWidth(),
        contentPadding = contentPadding,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (state.collisions.isNotEmpty()) {
            item(key = "collisions") {
                val first = state.collisions.first()
                Banner(
                    text = if (state.collisions.size == 1) {
                        "“${first.requestedPath}” arrived under a name that was taken."
                    } else {
                        "${state.collisions.size} items arrived under names that were taken."
                    },
                    action = {
                        TextButton(onClick = { onOpenCollision(first) }) { Text("Settle") }
                    },
                )
            }
        }

        if (state.items.isEmpty()) {
            item(key = "empty") {
                EmptyState(
                    icon = LocalCloudIcons.Files,
                    title = "Nothing shared yet",
                    body = if (state.paired.isEmpty()) {
                        "Add a file with the button below, then pair another device to send it there."
                    } else {
                        "Add a file with the button below. It stays on this device until you send it somewhere."
                    },
                )
            }
        } else {
            item(key = "header") {
                SectionHeader(
                    text = "Catalog",
                    trailing = "${state.items.size} ${if (state.items.size == 1) "item" else "items"}",
                )
            }

            items(state.items, key = { it.id }) { item ->
                ItemRow(
                    item = item,
                    transfers = transfers.values.filter { it.fileId == item.id },
                    pendingDeletes = state.deleteRequests
                        .filter { it.fileId == item.id }
                        .map { request ->
                            state.paired.firstOrNull { it.id == request.targetDevice }?.name
                                ?: request.targetDevice.take(8)
                        },
                    expanded = expandedId == item.id,
                    canSend = canSend,
                    onToggle = { expandedId = if (expandedId == item.id) null else item.id },
                    onShare = { onShare(item) },
                    onPull = { onPull(item.id) },
                    onDeleteHere = { onDeleteHere(item.id) },
                    onDeleteFrom = { deviceId -> onDeleteFrom(item.id, deviceId) },
                    modifier = Modifier.padding(horizontal = 16.dp),
                )
            }
        }
    }
}

@Composable
private fun ItemRow(
    item: Item,
    transfers: List<Transfer>,
    pendingDeletes: List<String>,
    expanded: Boolean,
    canSend: Boolean,
    onToggle: () -> Unit,
    onShare: () -> Unit,
    onPull: () -> Unit,
    onDeleteHere: () -> Unit,
    onDeleteFrom: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val size = rememberFormattedSize(item.size)

    Card(
        modifier = modifier
            .clickable(onClick = onToggle)
            .animateContentSize(),
        shape = RoundedCornerShape(16.dp),
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.surfaceContainer,
        ),
    ) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(
                text = item.name,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = buildString {
                    append(size)
                    append(" · ")
                    append(
                        when {
                            item.orphaned -> "no copies"
                            item.holderCount == 1 -> "on 1 device"
                            else -> "on ${item.holderCount} devices"
                        }
                    )
                    if (!item.heldHere) append(" · not on this device")
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(top = 2.dp),
            )

            HolderChips(
                holders = item.holders,
                modifier = Modifier.padding(top = 10.dp),
            )

            transfers.forEach { transfer -> TransferBar(transfer) }

            pendingDeletes.forEach { deviceName ->
                Text(
                    text = "Waiting to delete the copy on $deviceName.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 8.dp),
                )
            }

            AnimatedVisibility(visible = expanded) {
                Column {
                    HorizontalDivider(modifier = Modifier.padding(vertical = 12.dp))

                    Text(
                        text = "COPIES",
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )

                    if (item.orphaned) {
                        Text(
                            text = "The catalog still lists this item, but no device holds it.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier.padding(top = 8.dp),
                        )
                    }

                    item.holders.forEach { holder ->
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(top = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.SpaceBetween,
                        ) {
                            Text(
                                text = when {
                                    holder.isThisDevice -> "This device"
                                    holder.reachable -> holder.name
                                    else -> "${holder.name} (not on the network)"
                                },
                                style = MaterialTheme.typography.bodyMedium,
                                modifier = Modifier.weight(1f),
                            )
                            TextButton(
                                onClick = {
                                    if (holder.isThisDevice) onDeleteHere() else onDeleteFrom(holder.deviceId)
                                }
                            ) {
                                Text(if (holder.isThisDevice) "Delete here" else "Remove")
                            }
                        }
                    }

                    Row(
                        modifier = Modifier.padding(top = 4.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        if (item.heldHere) {
                            TextButton(onClick = onShare, enabled = canSend) {
                                Text("Send to a device")
                            }
                        } else {
                            TextButton(
                                onClick = onPull,
                                enabled = item.holders.any { it.reachable && !it.isThisDevice },
                            ) {
                                Text("Take a copy")
                            }
                        }
                    }

                    if (!item.heldHere && item.holders.none { it.reachable }) {
                        Text(
                            text = "No device holding this is on the network right now.",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

/**
 * One bar per destination.
 *
 * The engine counts blocks that actually have to move, so a file the other
 * device largely has already shows a short bar that finishes - not a long one
 * that jumps to the end. The count is shown alongside for that reason: it
 * explains a bar that only had four blocks to move.
 */
@Composable
private fun TransferBar(transfer: Transfer) {
    Column(modifier = Modifier.padding(top = 10.dp)) {
        Text(
            text = when (transfer.direction) {
                Transfer.Direction.Sending -> "Sending · ${transfer.blocksDone} of ${transfer.blocksTotal} blocks"
                Transfer.Direction.Receiving -> "Receiving · ${transfer.blocksDone} of ${transfer.blocksTotal} blocks"
            },
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        LinearProgressIndicator(
            progress = { transfer.fraction },
            modifier = Modifier
                .fillMaxWidth()
                .padding(top = 4.dp),
        )
    }
}
