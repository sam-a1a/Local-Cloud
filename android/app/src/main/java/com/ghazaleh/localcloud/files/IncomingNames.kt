package com.ghazaleh.localcloud.files

/**
 * A name the engine will accept, from whatever another app called the file.
 *
 * The engine refuses a name that is empty, hidden, or a path in disguise, and
 * it is right to - it is about to create a file in the sync folder and "../.."
 * is not a filename. But a rejected import is a poor way to find that out, and
 * since the share sheet arrived the names come from arbitrary apps rather than
 * from a document picker, so they are worth cleaning before they get there.
 */
internal fun sanitiseFileName(reported: String?): String {
    val candidate = reported.orEmpty()
        // Both separators, because a name can arrive from anywhere and a
        // Windows path pasted into a share is still a path.
        .substringAfterLast('/')
        .substringAfterLast('\\')
        .trim()
        // A leading dot means hidden, which is not what someone sharing a file
        // meant, and is one of the things the engine refuses outright.
        .trimStart('.')

    if (candidate.isBlank()) return FALLBACK_NAME
    if (candidate.length <= MAX_NAME_CHARS) return candidate

    // Truncated from the middle rather than the end, so the extension - the
    // part that decides whether the file opens in anything - survives.
    val extension = candidate.substringAfterLast('.', "")
    if (extension.isEmpty() || extension.length > MAX_EXTENSION_CHARS) {
        return candidate.take(MAX_NAME_CHARS)
    }
    return candidate.take(MAX_NAME_CHARS - extension.length - 1) + "." + extension
}

/** Long enough for any real name, short enough to survive a filesystem. */
private const val MAX_NAME_CHARS = 200

/** Beyond this it is not an extension, it is the rest of the name. */
private const val MAX_EXTENSION_CHARS = 16

internal const val FALLBACK_NAME = "Imported file"
