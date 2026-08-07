package com.ghazaleh.localcloud.ui.components

import android.text.format.Formatter
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.ghazaleh.localcloud.engine.Holder

/**
 * Who holds a copy, as a row of chips.
 *
 * The signature element of the app, and the one thing no ordinary file list
 * has to show. Cool means here, warm means elsewhere, and hollow means a device
 * that holds a copy but is not on the network to give it to you - which is the
 * difference between a file you can open and one you can only ask for.
 */
@Composable
fun HolderChips(
    holders: List<Holder>,
    modifier: Modifier = Modifier,
) {
    if (holders.isEmpty()) {
        Chip(
            text = "No copies anywhere",
            container = Color.Transparent,
            content = MaterialTheme.colorScheme.error,
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.error),
            modifier = modifier,
        )
        return
    }

    Row(
        modifier = modifier,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        holders.forEach { holder ->
            when {
                holder.isThisDevice -> Chip(
                    text = "This device",
                    container = MaterialTheme.colorScheme.primaryContainer,
                    content = MaterialTheme.colorScheme.onPrimaryContainer,
                )

                holder.reachable -> Chip(
                    text = holder.name,
                    container = MaterialTheme.colorScheme.tertiaryContainer,
                    content = MaterialTheme.colorScheme.onTertiaryContainer,
                )

                else -> Chip(
                    text = holder.name,
                    container = Color.Transparent,
                    content = MaterialTheme.colorScheme.onSurfaceVariant,
                    border = BorderStroke(1.dp, MaterialTheme.colorScheme.outline),
                )
            }
        }
    }
}

@Composable
fun Chip(
    text: String,
    container: Color,
    content: Color,
    modifier: Modifier = Modifier,
    border: BorderStroke? = null,
) {
    Surface(
        modifier = modifier,
        shape = RoundedCornerShape(50),
        color = container,
        contentColor = content,
        border = border,
    ) {
        Text(
            text = text,
            style = MaterialTheme.typography.labelMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(horizontal = 10.dp, vertical = 4.dp),
        )
    }
}

/** Present, or not. Used for a device on the network and for the engine itself. */
@Composable
fun StatusDot(
    on: Boolean,
    modifier: Modifier = Modifier,
) {
    Surface(
        modifier = modifier.size(8.dp),
        shape = CircleShape,
        color = if (on) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline,
        content = {},
    )
}

@Composable
fun SectionHeader(
    text: String,
    modifier: Modifier = Modifier,
    trailing: String? = null,
) {
    Row(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 20.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = text.uppercase(),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        if (trailing != null) {
            Text(
                text = trailing,
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/**
 * What a screen says when it has nothing.
 *
 * Every empty state here names the next action, because on first run all three
 * screens are empty at once and "no files" on its own tells someone nothing
 * about what this app is waiting for.
 */
@Composable
fun EmptyState(
    icon: ImageVector,
    title: String,
    body: String,
    modifier: Modifier = Modifier,
) {
    Column(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 40.dp, vertical = 56.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(
            imageVector = icon,
            contentDescription = null,
            tint = MaterialTheme.colorScheme.outline,
            modifier = Modifier.size(40.dp),
        )
        Text(
            text = title,
            style = MaterialTheme.typography.titleMedium,
            textAlign = TextAlign.Center,
        )
        Text(
            text = body,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
    }
}

/** A card that says something is wrong, or needs a decision, above a list. */
@Composable
fun Banner(
    text: String,
    modifier: Modifier = Modifier,
    container: Color = MaterialTheme.colorScheme.tertiaryContainer,
    content: Color = MaterialTheme.colorScheme.onTertiaryContainer,
    action: @Composable (() -> Unit)? = null,
) {
    Surface(
        modifier = modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 8.dp),
        shape = RoundedCornerShape(12.dp),
        color = container,
        contentColor = content,
    ) {
        Row(
            modifier = Modifier.padding(start = 16.dp, end = 8.dp, top = 12.dp, bottom = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                text = text,
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.weight(1f),
            )
            if (action != null) {
                Spacer(Modifier.width(8.dp))
                action()
            }
        }
    }
}

/** The platform's own idea of a file size, so it matches everything else on the phone. */
@Composable
fun rememberFormattedSize(bytes: Long): String {
    val context = LocalContext.current
    return Formatter.formatShortFileSize(context, bytes)
}

/**
 * How long a trashed item has left.
 *
 * Coarse on purpose: the retention is 30 days, and the difference between 29
 * days and 29 days 4 hours is not a difference anyone acts on. It sharpens as
 * it runs out, which is when it starts to matter.
 */
fun formatRemaining(seconds: Long?): String = when {
    seconds == null -> "No longer counting down"
    seconds <= 0L -> "Due to be destroyed"
    seconds >= 172_800L -> "${seconds / 86_400L} days left"
    seconds >= 86_400L -> "1 day left"
    seconds >= 7_200L -> "${seconds / 3_600L} hours left"
    seconds >= 3_600L -> "1 hour left"
    seconds >= 120L -> "${seconds / 60L} minutes left"
    else -> "Less than a minute left"
}
