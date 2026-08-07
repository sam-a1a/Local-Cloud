package com.ghazaleh.localcloud.ui.theme

import androidx.compose.ui.graphics.Color

/**
 * Three roles carry nearly all the meaning in this app, so the palette is built
 * around them rather than around a brand colour.
 *
 * - **Primary**, a deep teal, is this device and the actions it can take.
 * - **Tertiary**, a warm copper, is *another* device. Every place the UI has to
 *   say "over there" - a holder chip for a peer, an incoming transfer - reaches
 *   for this, so the distinction between here and elsewhere is a colour rather
 *   than a label you have to read.
 * - **Outline** and the surface variants are absence: an item in the catalog
 *   that this device does not hold.
 *
 * Cool for what is yours, warm for what is not. That single opposition is what
 * makes a row of holder chips readable at a glance.
 */

// Light.
val TealPrimaryLight = Color(0xFF1E6C74)
val TealOnPrimaryLight = Color(0xFFFFFFFF)
val TealContainerLight = Color(0xFFA6EDF6)
val TealOnContainerLight = Color(0xFF002F34)

val SlateSecondaryLight = Color(0xFF4A6265)
val SlateOnSecondaryLight = Color(0xFFFFFFFF)
val SlateContainerLight = Color(0xFFCCE7EA)
val SlateOnContainerLight = Color(0xFF051F22)

val CopperTertiaryLight = Color(0xFF8A5100)
val CopperOnTertiaryLight = Color(0xFFFFFFFF)
val CopperContainerLight = Color(0xFFFFDCBC)
val CopperOnContainerLight = Color(0xFF2C1600)

val SurfaceLight = Color(0xFFF5FAFB)
val OnSurfaceLight = Color(0xFF171D1E)
val SurfaceVariantLight = Color(0xFFDBE4E6)
val OnSurfaceVariantLight = Color(0xFF3F484A)
val OutlineLight = Color(0xFF6F797A)

// Dark.
val TealPrimaryDark = Color(0xFF82D3DC)
val TealOnPrimaryDark = Color(0xFF00363C)
val TealContainerDark = Color(0xFF004F58)
val TealOnContainerDark = Color(0xFF9EECF6)

val SlateSecondaryDark = Color(0xFFB0CBCE)
val SlateOnSecondaryDark = Color(0xFF1B3437)
val SlateContainerDark = Color(0xFF324B4E)
val SlateOnContainerDark = Color(0xFFCCE7EA)

val CopperTertiaryDark = Color(0xFFFFB870)
val CopperOnTertiaryDark = Color(0xFF4A2800)
val CopperContainerDark = Color(0xFF693C00)
val CopperOnContainerDark = Color(0xFFFFDCBC)

val SurfaceDark = Color(0xFF0E1415)
val OnSurfaceDark = Color(0xFFDDE4E5)
val SurfaceVariantDark = Color(0xFF3F484A)
val OnSurfaceVariantDark = Color(0xFFBFC8CA)
val OutlineDark = Color(0xFF899294)
