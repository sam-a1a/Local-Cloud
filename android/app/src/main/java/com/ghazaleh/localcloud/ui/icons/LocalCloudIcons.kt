package com.ghazaleh.localcloud.ui.icons

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.unit.dp

/**
 * The few icons this app actually needs, drawn here.
 *
 * Not an aesthetic preference so much as an avoided dependency: the Material
 * icon artifacts are a large library to carry for six glyphs, and the ones in
 * the core set are a fixed vocabulary that has no symbol for "these devices
 * hold this file". Everywhere an icon would be a guess, this app uses a word
 * instead - actions on a row are labelled buttons, because "Take a copy" is
 * unambiguous and a downward arrow is not.
 *
 * Stroked rather than filled, at a consistent 1.8 weight, so they sit with
 * Material 3's outlined style rather than fighting it. [androidx.compose.material3.Icon]
 * tints the whole vector, so drawing in black here is only a placeholder.
 */
object LocalCloudIcons {

    /** Documents in the shared catalog. */
    val Files: ImageVector by lazy {
        strokeIcon(
            "Files",
            "M6 3 H14 L18.5 7.5 V21 H6 Z",
            "M14 3 V7.5 H18.5",
        )
    }

    /** A big screen and a small one: the mesh. */
    val Devices: ImageVector by lazy {
        strokeIcon(
            "Devices",
            "M3 6 H14.5 V14 H3 Z",
            "M8.75 14 V17.5",
            "M6.5 17.5 H11",
            "M17.5 9.5 H21 V18 H17.5 Z",
        )
    }

    val Trash: ImageVector by lazy {
        strokeIcon(
            "Trash",
            "M4.5 6.5 H19.5",
            "M9.5 6.5 V4 H14.5 V6.5",
            "M6.5 6.5 L7.5 20.5 H16.5 L17.5 6.5",
            "M10.5 10.5 V16.5",
            "M13.5 10.5 V16.5",
        )
    }

    val Add: ImageVector by lazy {
        strokeIcon(
            "Add",
            "M12 5 V19",
            "M5 12 H19",
        )
    }

    val Close: ImageVector by lazy {
        strokeIcon(
            "Close",
            "M6.5 6.5 L17.5 17.5",
            "M17.5 6.5 L6.5 17.5",
        )
    }

    val Check: ImageVector by lazy {
        strokeIcon(
            "Check",
            "M5 12.5 L9.5 17 L19 6.5",
        )
    }

    val Warning: ImageVector by lazy {
        strokeIcon(
            "Warning",
            "M12 4 L21 20 H3 Z",
            "M12 10 V14",
            "M12 16.9 V17",
        )
    }

    /** Pairing: two halves of one chain. */
    val Link: ImageVector by lazy {
        strokeIcon(
            "Link",
            "M10 8 H7.5 A 4 4 0 0 0 7.5 16 H10",
            "M14 8 H16.5 A 4 4 0 0 1 16.5 16 H14",
            "M8.5 12 H15.5",
        )
    }
}

private fun strokeIcon(name: String, vararg paths: String): ImageVector =
    ImageVector.Builder(
        name = name,
        defaultWidth = 24.dp,
        defaultHeight = 24.dp,
        viewportWidth = 24f,
        viewportHeight = 24f,
    ).apply {
        paths.forEach { data ->
            addPath(
                pathData = addPathNodes(data),
                fill = null,
                stroke = SolidColor(Color.Black),
                strokeLineWidth = 1.8f,
                strokeLineCap = StrokeCap.Round,
                strokeLineJoin = StrokeJoin.Round,
            )
        }
    }.build()
