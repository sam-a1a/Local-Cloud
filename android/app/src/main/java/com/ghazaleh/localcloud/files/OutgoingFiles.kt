package com.ghazaleh.localcloud.files

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import java.io.File
import java.util.Locale

/**
 * How a file gets out of this app.
 *
 * Everything the mesh delivers lands in app-private storage, which is what
 * keeps it private and also what makes it useless on its own - a photo that
 * arrives from another device and cannot be looked at is not a photo that
 * arrived. This is the way back out: a content URI, granted for one file and
 * one launch at a time.
 *
 * Nothing here copies bytes. The receiving app reads the file where it lies,
 * which matters when the file is a video.
 */
object OutgoingFiles {

    /**
     * A URI another app can read, valid only while the intent carrying it is.
     *
     * Throws if the file is outside the directory the provider is configured
     * with, which is the point of the provider: the set of files that can be
     * handed out is decided by `file_paths.xml`, not by the caller.
     */
    fun uriFor(context: Context, file: File): Uri =
        FileProvider.getUriForFile(context, "${context.packageName}.fileprovider", file)

    /** Opens the file in whatever the phone uses for that kind of thing. */
    fun viewIntent(context: Context, file: File): Intent =
        Intent(Intent.ACTION_VIEW)
            .setDataAndType(uriFor(context, file), mimeTypeOf(file.name))
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)

    /** Passes the file on - to a chat, to Drive, to anything that takes files. */
    fun sendIntent(context: Context, file: File): Intent =
        Intent(Intent.ACTION_SEND)
            .setType(mimeTypeOf(file.name))
            .putExtra(Intent.EXTRA_STREAM, uriFor(context, file))
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)

    /**
     * The type, guessed from the name, because that is all there is to go on.
     *
     * The engine stores bytes and a name and has no opinion about formats -
     * correctly, since a mesh of your own devices should not care what you put
     * in it. Falling back to `application/octet-stream` means an unrecognised
     * file still opens a chooser rather than nothing at all.
     */
    fun mimeTypeOf(fileName: String): String {
        val extension = fileName.substringAfterLast('.', "").lowercase(Locale.US)
        if (extension.isEmpty()) return FALLBACK
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension) ?: FALLBACK
    }

    private const val FALLBACK = "application/octet-stream"
}
