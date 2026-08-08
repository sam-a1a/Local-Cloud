package com.ghazaleh.localcloud.files

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Names arriving from other apps.
 *
 * This mattered less when the only way in was a document picker, which supplies
 * sensible names. The share sheet is open to anything on the phone, and the
 * engine refuses a name that is empty, hidden, or a path in disguise - so
 * everything it refuses should be dealt with before it gets there, and the
 * cases where that goes wrong are exactly the ones nobody thinks to try.
 */
class IncomingNamesTest {

    @Test
    fun `an ordinary name is left alone`() {
        assertEquals("photo.jpg", sanitiseFileName("photo.jpg"))
    }

    @Test
    fun `a path is reduced to the file at the end of it`() {
        assertEquals("photo.jpg", sanitiseFileName("/storage/emulated/0/DCIM/photo.jpg"))
        assertEquals("photo.jpg", sanitiseFileName("C:\\Users\\sam\\photo.jpg"))
    }

    @Test
    fun `surrounding space is not part of a name`() {
        assertEquals("photo.jpg", sanitiseFileName("   photo.jpg  "))
    }

    @Test
    fun `a leading dot is dropped, because nobody shares a file to hide it`() {
        assertEquals("profile", sanitiseFileName(".profile"))
        assertEquals("hidden.txt", sanitiseFileName("...hidden.txt"))
    }

    @Test
    fun `nothing usable falls back rather than failing the import`() {
        assertEquals(FALLBACK_NAME, sanitiseFileName(null))
        assertEquals(FALLBACK_NAME, sanitiseFileName(""))
        assertEquals(FALLBACK_NAME, sanitiseFileName("   "))
        assertEquals(FALLBACK_NAME, sanitiseFileName("..."))
        assertEquals(FALLBACK_NAME, sanitiseFileName("/a/b/"))
    }

    @Test
    fun `an absurd name is shortened but keeps the part that decides what opens it`() {
        val absurd = "x".repeat(400) + ".png"

        val cleaned = sanitiseFileName(absurd)

        assertTrue("was ${cleaned.length} characters", cleaned.length <= 200)
        assertTrue("lost the extension: $cleaned", cleaned.endsWith(".png"))
    }

    @Test
    fun `something that is all extension is truncated rather than trusted`() {
        // ".gpg.enc.backup.2026" is not an extension, and treating it as one
        // would leave almost nothing of the name.
        val absurd = "y".repeat(400) + "." + "z".repeat(100)

        val cleaned = sanitiseFileName(absurd)

        assertEquals(200, cleaned.length)
    }
}
