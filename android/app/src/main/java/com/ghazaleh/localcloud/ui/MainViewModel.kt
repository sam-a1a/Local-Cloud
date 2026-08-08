package com.ghazaleh.localcloud.ui

import android.app.Application
import android.net.Uri
import android.provider.OpenableColumns
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.ghazaleh.localcloud.LocalCloudApplication
import com.ghazaleh.localcloud.engine.Item
import com.ghazaleh.localcloud.engine.describeForUser
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.localcloud.CollisionResolution
import uniffi.localcloud.DiscoveredDevice
import uniffi.localcloud.PairingOffer
import uniffi.localcloud.PendingCollision
import java.io.File

/**
 * Where a tap becomes an engine call.
 *
 * The engine's own state is read straight off the repository - re-exposing it
 * here would only be a second copy to keep true. What this adds is the state
 * that exists solely because there is a screen: which dialog is open, what has
 * been typed into it, and which row is expanded.
 */
class MainViewModel(application: Application) : AndroidViewModel(application) {

    private val repository = (application as LocalCloudApplication).repository
    private val preferences = (application as LocalCloudApplication).preferences

    /** Whether the person has asked this device to keep syncing with the app closed. */
    val backgroundSync = preferences.backgroundSync

    val state = repository.state
    val transfers = repository.transfers
    val notices = repository.notices

    private val _pairing = MutableStateFlow<PairingFlow>(PairingFlow.None)
    val pairing = _pairing.asStateFlow()

    private val _sharing = MutableStateFlow<Item?>(null)
    val sharing = _sharing.asStateFlow()

    private val _collision = MutableStateFlow<PendingCollision?>(null)
    val collision = _collision.asStateFlow()

    private val _importing = MutableStateFlow(false)
    val importing = _importing.asStateFlow()

    private val _renaming = MutableStateFlow<RenameDraft?>(null)
    val renaming = _renaming.asStateFlow()

    init {
        // The device that shows the code never presses anything to finish:
        // pairing completes when the *other* device enters it, and arrives here
        // as a change in the paired list. Close the dialog when that happens,
        // rather than leaving a code on screen that no longer means anything.
        viewModelScope.launch {
            state.collect { mesh ->
                val showing = _pairing.value
                if (showing is PairingFlow.ShowingCode &&
                    mesh.paired.any { it.id == showing.device.deviceId }
                ) {
                    _pairing.value = PairingFlow.None
                }
            }
        }
    }

    // -- Files --------------------------------------------------------------

    /**
     * Brings a file in from the document picker.
     *
     * A `content://` URI is not a path, and the engine takes a path - so the
     * bytes are copied to the cache first and handed over from there. The engine
     * copies again, into the sync folder, which is why the temporary file is
     * deleted immediately afterwards rather than left for the cache to reap.
     */
    fun importFrom(uri: Uri) = importAll(listOf(uri))

    /**
     * Brings in everything that arrived at once.
     *
     * The share sheet can hand over several files in one go, and importing them
     * one at a time with a spinner flickering between each would be a poorer
     * report of the same work. Failures are per file - one unreadable item does
     * not abandon the rest - and the engine has already said why for each.
     */
    fun importAll(uris: List<Uri>) {
        if (uris.isEmpty()) return
        viewModelScope.launch {
            _importing.value = true
            try {
                val added = uris.count { importOne(it) }
                if (uris.size > 1) {
                    repository.report("Added $added of ${uris.size} files.")
                }
            } finally {
                _importing.value = false
            }
        }
    }

    private suspend fun importOne(uri: Uri): Boolean {
        val context = getApplication<Application>()
        val staged = try {
            withContext(Dispatchers.IO) {
                val temporary = File.createTempFile("import", null, context.cacheDir)
                context.contentResolver.openInputStream(uri).use { input ->
                    requireNotNull(input) { "That file could not be opened." }
                    temporary.outputStream().use(input::copyTo)
                }
                temporary
            }
        } catch (t: Throwable) {
            repository.report(t.describeForUser(), failure = true)
            return false
        }

        return try {
            repository.importFile(staged.absolutePath, sanitizedName(uri))
        } finally {
            withContext(Dispatchers.IO) { staged.delete() }
        }
    }

    /**
     * The catalog said this device holds it and the disk disagreed.
     *
     * Rare, and worth a sentence rather than silence: it means the copy went
     * away without the engine being told, which is not something the app can
     * fix but is something a person should know before they go looking.
     */
    fun reportFileUnavailable(name: String) {
        repository.report("“$name” is not on this device any more.", failure = true)
    }

    fun beginSharing(item: Item) {
        _sharing.value = item
    }

    fun stopSharing() {
        _sharing.value = null
    }

    fun share(fileId: String, deviceIds: List<String>) {
        _sharing.value = null
        viewModelScope.launch { repository.share(fileId, deviceIds) }
    }

    fun pull(fileId: String) {
        viewModelScope.launch { repository.pull(fileId) }
    }

    fun deleteHere(fileId: String) {
        viewModelScope.launch { repository.deleteHere(fileId) }
    }

    fun deleteFrom(fileId: String, deviceId: String) {
        viewModelScope.launch { repository.deleteFrom(fileId, deviceId) }
    }

    // -- Trash --------------------------------------------------------------

    fun restore(fileId: String) {
        viewModelScope.launch { repository.restore(fileId) }
    }

    fun destroy(fileId: String) {
        viewModelScope.launch { repository.destroy(fileId) }
    }

    // -- Contested names ----------------------------------------------------

    fun openCollision(collision: PendingCollision) {
        _collision.value = collision
    }

    fun dismissCollision() {
        _collision.value = null
    }

    fun resolveCollision(collisionId: String, resolution: CollisionResolution) {
        _collision.value = null
        viewModelScope.launch { repository.resolveCollision(collisionId, resolution) }
    }

    // -- Pairing ------------------------------------------------------------

    fun startPairing(device: DiscoveredDevice) {
        _pairing.value = PairingFlow.Requesting(device)
        viewModelScope.launch {
            val code = repository.beginPairing(device.deviceId)
            _pairing.value =
                if (code == null) PairingFlow.None else PairingFlow.ShowingCode(device, code)
        }
    }

    fun openOffer(offer: PairingOffer) {
        _pairing.value = PairingFlow.Entering(offer)
    }

    fun typeCode(digits: String) {
        _pairing.update { current ->
            if (current is PairingFlow.Entering) {
                current.copy(code = digits.filter(Char::isDigit).take(CODE_LENGTH))
            } else {
                current
            }
        }
    }

    fun submitCode() {
        val entering = _pairing.value as? PairingFlow.Entering ?: return
        _pairing.value = entering.copy(busy = true)
        viewModelScope.launch {
            val paired = repository.confirmPairing(entering.offer.deviceId, entering.code)
            _pairing.value = if (paired) PairingFlow.None else entering.copy(busy = false, code = "")
        }
    }

    fun dismissPairing() {
        val current = _pairing.value
        _pairing.value = PairingFlow.None
        // Only the side that offered a code has something to call off.
        if (current is PairingFlow.ShowingCode || current is PairingFlow.Requesting) {
            viewModelScope.launch { repository.cancelPairing() }
        }
    }

    fun unpair(deviceId: String) {
        viewModelScope.launch { repository.unpair(deviceId) }
    }

    // -- This device --------------------------------------------------------

    /**
     * Records the choice. Starting and stopping the service follows from it.
     *
     * The setting is what is stored, not the fact of a running service: the
     * system may kill and restart the service on its own, and the switch should
     * keep reporting what was asked for rather than flickering with it.
     */
    fun setBackgroundSync(enabled: Boolean) {
        preferences.setBackgroundSync(enabled)
    }

    /**
     * The switch stays off if the notification was refused.
     *
     * The service would run either way — Android does not require the
     * notification to be *visible*, only posted — and that is exactly the
     * outcome to refuse: a device syncing with the screen off and nothing
     * anywhere saying it is.
     */
    fun reportNotificationsRefused() {
        repository.report(
            "Background syncing needs a notification, so you can see when this device is on the mesh.",
            failure = true,
        )
    }

    fun beginRename() {
        _renaming.value = RenameDraft(state.value.thisDevice.name)
    }

    fun typeName(text: String) {
        _renaming.update { draft -> draft?.copy(text = text.take(MAX_NAME_CHARS)) }
    }

    /**
     * The engine is the authority on whether a name is usable, not this.
     *
     * The field is capped at the same length the engine accepts so that the
     * common rejection cannot happen at all, but everything else - a name of
     * only spaces, a stray newline from a paste - is left to `set_device_name`
     * to refuse and to say why.
     */
    fun submitRename() {
        val draft = _renaming.value ?: return
        _renaming.value = draft.copy(busy = true)
        viewModelScope.launch {
            val renamed = repository.rename(draft.text)
            _renaming.value = if (renamed) null else draft.copy(busy = false)
        }
    }

    fun dismissRename() {
        _renaming.value = null
    }

    private fun sanitizedName(uri: Uri): String {
        val fromProvider = getApplication<Application>().contentResolver
            .query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { cursor ->
                if (cursor.moveToFirst() && !cursor.isNull(0)) cursor.getString(0) else null
            }

        // The engine refuses a name that is empty, hidden, or a path in
        // disguise, and it is right to - but a rejected import is a poor way to
        // find that out, so the few characters it objects to are dealt with
        // here instead.
        val candidate = (fromProvider ?: uri.lastPathSegment.orEmpty())
            .substringAfterLast('/')
            .substringAfterLast('\\')
            .trim()
            .trimStart('.')

        return candidate.ifBlank { FALLBACK_NAME }
    }

    private companion object {
        const val CODE_LENGTH = 6
        const val FALLBACK_NAME = "Imported file"

        /** What the engine accepts, so the field cannot offer what it will refuse. */
        const val MAX_NAME_CHARS = 64
    }
}

/** What is being typed into the rename dialog. */
data class RenameDraft(val text: String, val busy: Boolean = false)

/** Which part of pairing, if any, is on screen. */
sealed interface PairingFlow {

    data object None : PairingFlow

    /** The code has been asked for and has not come back yet. */
    data class Requesting(val device: DiscoveredDevice) : PairingFlow

    /** This device offered; the six digits are waiting to be read off it. */
    data class ShowingCode(val device: DiscoveredDevice, val code: String) : PairingFlow

    /** The other device offered; the six digits are being typed in here. */
    data class Entering(
        val offer: PairingOffer,
        val code: String = "",
        val busy: Boolean = false,
    ) : PairingFlow
}
