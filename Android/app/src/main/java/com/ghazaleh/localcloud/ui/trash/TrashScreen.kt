package com.ghazaleh.localcloud.ui.trash

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
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
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
import com.ghazaleh.localcloud.engine.TrashItem
import com.ghazaleh.localcloud.ui.components.EmptyState
import com.ghazaleh.localcloud.ui.components.SectionHeader
import com.ghazaleh.localcloud.ui.components.formatRemaining
import com.ghazaleh.localcloud.ui.components.rememberFormattedSize
import com.ghazaleh.localcloud.ui.icons.LocalCloudIcons

/**
 * Items nobody holds any more, and the 30 days before that becomes permanent.
 *
 * Only the last copy of an item ever lands here. Deleting a copy while others
 * remain frees the space immediately, because nothing can be lost - so a
 * screen with anything on it is a screen of things that would otherwise be
 * gone, which is why the countdown is the most prominent thing on each row.
 */
@Composable
fun TrashScreen(
    state: MeshState,
    onRestore: (String) -> Unit,
    onDestroy: (String) -> Unit,
    contentPadding: PaddingValues,
    modifier: Modifier = Modifier,
) {
    var destroying by remember { mutableStateOf<TrashItem?>(null) }

    LazyColumn(
        modifier = modifier.fillMaxWidth(),
        contentPadding = contentPadding,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (state.trash.isEmpty()) {
            item(key = "empty") {
                EmptyState(
                    icon = LocalCloudIcons.Trash,
                    title = "Trash is empty",
                    body = "An item comes here when its last copy is deleted. " +
                        "Until then, deleting a copy just frees the space on that device.",
                )
            }
        } else {
            item(key = "header") {
                SectionHeader(
                    text = "Recoverable",
                    trailing = "${state.trash.size}",
                )
            }
            items(state.trash, key = { it.id }) { entry ->
                TrashRow(
                    entry = entry,
                    onRestore = { onRestore(entry.id) },
                    onDestroy = { destroying = entry },
                    modifier = Modifier.padding(horizontal = 16.dp),
                )
            }
        }
    }

    val target = destroying
    if (target != null) {
        AlertDialog(
            onDismissRequest = { destroying = null },
            title = { Text("Destroy “${target.name}”?") },
            text = {
                Text(
                    "The bytes are released and a tombstone travels to every paired device, " +
                        "so it will not come back from any of them. This cannot be undone."
                )
            },
            confirmButton = {
                TextButton(
                    onClick = {
                        onDestroy(target.id)
                        destroying = null
                    }
                ) { Text("Destroy") }
            },
            dismissButton = {
                TextButton(onClick = { destroying = null }) { Text("Keep") }
            },
        )
    }
}

@Composable
private fun TrashRow(
    entry: TrashItem,
    onRestore: () -> Unit,
    onDestroy: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val size = rememberFormattedSize(entry.size)

    Card(
        modifier = modifier.fillMaxWidth(),
        shape = RoundedCornerShape(16.dp),
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.4f),
        ),
    ) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(
                text = entry.name,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                text = "$size · deleted by ${entry.trashedBy}",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(top = 2.dp),
            )
            Text(
                text = formatRemaining(entry.secondsRemaining),
                style = MaterialTheme.typography.titleSmall,
                color = MaterialTheme.colorScheme.tertiary,
                modifier = Modifier.padding(top = 8.dp),
            )
            Row(
                modifier = Modifier.padding(top = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                TextButton(onClick = onRestore) { Text("Restore") }
                TextButton(onClick = onDestroy) { Text("Destroy now") }
            }
        }
    }
}
