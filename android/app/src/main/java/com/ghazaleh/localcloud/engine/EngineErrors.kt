package com.ghazaleh.localcloud.engine

import uniffi.localcloud.EngineException

/**
 * What to put on screen when the engine refuses.
 *
 * The engine writes a sentence for every one of these - "That device is not
 * visible on the network" - but the sentence does not survive the crossing.
 * uniffi generates `message` from the variant's *fields*, so what actually
 * arrives in Kotlin is `deviceId=7f3a...`, which is diagnostics, not something
 * to show a person. The typed variants do survive, and they were the point:
 * this matches on them and supplies the English on this side.
 *
 * Deliberately not a `when(e.message)`. The variant is the contract; the
 * wording is ours to improve.
 */
fun EngineException.describe(): String = when (this) {
    is EngineException.NoSuchItem -> "That item is no longer in the catalog."
    is EngineException.NotHeldHere -> "This device does not have that file's contents."
    is EngineException.NotAHolder -> "That device does not have a copy to work with."
    is EngineException.NotTrashed -> "Delete the remaining copies before destroying this."
    is EngineException.InTrash -> "That item is in the trash. Restore it first."
    is EngineException.NotPaired -> "That device is not paired with this one."
    is EngineException.NotVisible -> "That device is not on the network right now."
    is EngineException.InvalidName -> "“$name” cannot be used as a name."
    is EngineException.NoSuchFile -> "The file could not be read."
    is EngineException.NothingSelected -> "Choose at least one device."
    is EngineException.NoUsableDevices -> reason
    is EngineException.NoSuchCollision -> "That name conflict was already settled."
    is EngineException.Pairing -> reason
    is EngineException.Internal -> "Something failed on this device: $reason"
}

/**
 * Anything at all that went wrong, as a sentence.
 *
 * Engine failures get the wording above; anything else - a content resolver
 * that will not open a URI, a full disk while copying - falls back to its own
 * message rather than being reported as an engine problem it was not.
 */
fun Throwable.describeForUser(): String = when (this) {
    is EngineException -> describe()
    else -> message?.takeIf { it.isNotBlank() } ?: "Something went wrong."
}
