package com.ghazaleh.localcloud.ui.theme

import androidx.compose.material3.Typography
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

val Typography = Typography()

/**
 * The six digits, set to be read aloud across a room.
 *
 * Monospaced and widely tracked because this is the one piece of text in the
 * app that someone copies by eye onto another device, and the failure it has to
 * design against is a misread digit, not an ugly one.
 */
val PairingCodeStyle = TextStyle(
    fontFamily = FontFamily.Monospace,
    fontWeight = FontWeight.Medium,
    fontSize = 44.sp,
    letterSpacing = 12.sp,
)

/** Device ids and content hashes: never read as words, only compared. */
val IdentifierStyle = TextStyle(
    fontFamily = FontFamily.Monospace,
    fontSize = 12.sp,
    letterSpacing = 0.5.sp,
)
