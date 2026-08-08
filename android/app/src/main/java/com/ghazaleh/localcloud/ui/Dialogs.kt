package com.ghazaleh.localcloud.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.ghazaleh.localcloud.engine.Item
import com.ghazaleh.localcloud.ui.theme.PairingCodeStyle
import uniffi.localcloud.CollisionResolution
import uniffi.localcloud.PairedDevice
import uniffi.localcloud.PendingCollision

/**
 * Pairing, from either side.
 *
 * One device shows six digits and the other types them in, so both halves are
 * here together - the asymmetry is the whole protocol, and separating them
 * would make it easy to write two screens that disagree about which is which.
 */
@Composable
fun PairingDialog(
    flow: PairingFlow,
    onType: (String) -> Unit,
    onSubmit: () -> Unit,
    onDismiss: () -> Unit,
) {
    when (flow) {
        PairingFlow.None -> Unit

        is PairingFlow.Requesting -> AlertDialog(
            onDismissRequest = onDismiss,
            title = { Text("Pairing with ${flow.device.name}") },
            text = {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    CircularProgressIndicator()
                    Text("Asking for a code…")
                }
            },
            confirmButton = {},
            dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
        )

        is PairingFlow.ShowingCode -> AlertDialog(
            onDismissRequest = onDismiss,
            title = { Text("Type this on ${flow.device.name}") },
            text = {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(
                        text = flow.code,
                        style = PairingCodeStyle,
                        color = MaterialTheme.colorScheme.primary,
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 20.dp),
                        textAlign = TextAlign.Center,
                    )
                    Text(
                        text = "This dialog closes by itself once the other device has entered it. " +
                            "The code is good for a few minutes and a handful of attempts.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                    )
                }
            },
            confirmButton = {},
            dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
        )

        is PairingFlow.Entering -> AlertDialog(
            onDismissRequest = onDismiss,
            title = { Text("Pair with ${flow.offer.name}") },
            text = {
                Column {
                    Text(
                        text = "Enter the six digits showing on ${flow.offer.name}.",
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    OutlinedTextField(
                        value = flow.code,
                        onValueChange = onType,
                        singleLine = true,
                        enabled = !flow.busy,
                        label = { Text("Code") },
                        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                            keyboardType = KeyboardType.NumberPassword,
                        ),
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 16.dp),
                    )
                }
            },
            confirmButton = {
                TextButton(
                    onClick = onSubmit,
                    enabled = flow.code.length == 6 && !flow.busy,
                ) { Text("Pair") }
            },
            dismissButton = { TextButton(onClick = onDismiss) { Text("Not now") } },
        )
    }
}

/**
 * Which devices should get a copy.
 *
 * Devices that already hold it are not offered, because sending a copy to a
 * device that has one is not an operation - and the engine would only reply
 * that it moved no blocks.
 */
@Composable
fun ShareDialog(
    item: Item,
    candidates: List<PairedDevice>,
    onlineIds: Set<String>,
    onSend: (List<String>) -> Unit,
    onDismiss: () -> Unit,
) {
    var selected by remember(item.id) { mutableStateOf(emptySet<String>()) }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Send “${item.name}”") },
        text = {
            if (candidates.isEmpty()) {
                Text("Every paired device already has a copy of this.")
            } else {
                Column(modifier = Modifier.verticalScroll(rememberScrollState())) {
                    candidates.forEach { device ->
                        val reachable = device.id in onlineIds
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable(enabled = reachable) {
                                    selected = if (device.id in selected) {
                                        selected - device.id
                                    } else {
                                        selected + device.id
                                    }
                                }
                                .padding(vertical = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Checkbox(
                                checked = device.id in selected,
                                onCheckedChange = null,
                                enabled = reachable,
                            )
                            Column(modifier = Modifier.padding(start = 12.dp)) {
                                Text(
                                    text = device.name,
                                    style = MaterialTheme.typography.bodyLarge,
                                )
                                if (!reachable) {
                                    Text(
                                        text = "Not on the network right now",
                                        style = MaterialTheme.typography.bodySmall,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    )
                                }
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(
                onClick = { onSend(selected.toList()) },
                enabled = selected.isNotEmpty(),
            ) { Text("Send") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/**
 * A name that arrived and was already taken.
 *
 * The engine has already done the safe thing - kept both, under a numbered
 * name - so this is not an emergency and the dialog does not present it as
 * one. It asks whether the safe thing was the right thing.
 */
@Composable
fun CollisionDialog(
    collision: PendingCollision,
    onResolve: (CollisionResolution) -> Unit,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Two items want the name “${collision.requestedPath}”") },
        text = {
            Text(
                "The one that arrived is being kept as “${collision.currentPath}” so that nothing " +
                    "was overwritten. Should it take the name instead, sending the existing item to the trash?"
            )
        },
        confirmButton = {
            TextButton(onClick = { onResolve(CollisionResolution.OVERRIDE) }) {
                Text("Take the name")
            }
        },
        dismissButton = {
            TextButton(onClick = { onResolve(CollisionResolution.KEEP_BOTH) }) {
                Text("Keep both")
            }
        },
    )
}

/**
 * Renaming this device.
 *
 * Worth having a screen for, rather than leaving the name to whatever the
 * platform reported: it is the only thing about this device that other people
 * see, and on a mesh of three phones "Pixel 7" twice is not a name at all.
 */
@Composable
fun RenameDialog(
    draft: RenameDraft,
    onType: (String) -> Unit,
    onSubmit: () -> Unit,
    onDismiss: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Name this device") },
        text = {
            Column {
                Text(
                    text = "This is what other devices call it, when they see it on the " +
                        "network and when they list who holds a file.",
                    style = MaterialTheme.typography.bodyMedium,
                )
                OutlinedTextField(
                    value = draft.text,
                    onValueChange = onType,
                    singleLine = true,
                    enabled = !draft.busy,
                    label = { Text("Name") },
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(top = 16.dp),
                )
            }
        },
        confirmButton = {
            TextButton(
                onClick = onSubmit,
                enabled = draft.text.isNotBlank() && !draft.busy,
            ) { Text("Rename") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
