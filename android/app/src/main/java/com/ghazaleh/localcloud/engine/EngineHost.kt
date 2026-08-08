package com.ghazaleh.localcloud.engine

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import android.provider.Settings
import com.ghazaleh.localcloud.Preferences
import uniffi.localcloud.Engine
import uniffi.localcloud.EventListener
import java.io.File

/**
 * Owns the one engine this process has, and the two Android-shaped things it
 * cannot work without: somewhere to keep its state, and permission to hear
 * multicast.
 *
 * §14 of DESIGN.md lists the multicast lock as the app's job rather than the
 * engine's, and this is why it has to be: Android silently drops multicast
 * packets not addressed to the device unless something holds a
 * [WifiManager.MulticastLock]. Without it mDNS is not slow or unreliable - it
 * returns nothing, forever, on a network where every other device can see each
 * other. It is the single most likely reason for "no devices found".
 */
class EngineHost(
    context: Context,
    private val preferences: Preferences,
) {

    private val appContext = context.applicationContext

    /**
     * Engine state - the device's private key, the catalog, and every block
     * this device holds - lives under `noBackupFilesDir`.
     *
     * That is a deliberate choice rather than a default. `filesDir` is included
     * in Android's automatic cloud backup, and restoring this app onto a second
     * phone would put the same device identity on two devices at once: two
     * members of the mesh with one id and one certificate, which is not a
     * device that moved but a device that forked. Recovering a phone is what
     * pairing and pulling copies are for, and they do it without ever
     * duplicating an identity.
     */
    private val baseDir = File(appContext.noBackupFilesDir, "engine")

    /**
     * What this device holds, as files.
     *
     * Still app-private - no storage permission, nothing else on the phone can
     * read the contents of the mesh - but under `filesDir` rather than beside
     * the engine's state in `noBackupFilesDir`, and that difference is the
     * whole reason a received file can be opened at all.
     *
     * A `FileProvider` can only grant access to a directory it has been
     * configured with, and its vocabulary is `files-path`, `cache-path` and the
     * external ones. There is no tag for `no_backup`. The alternatives were
     * naming the private data path literally, which differs per user and work
     * profile, or copying every file to the cache before handing it out, which
     * for a video means duplicating gigabytes to open one. Moving the directory
     * costs nothing and both problems disappear.
     *
     * Backup is then handled explicitly rather than by location: see
     * `backup_rules.xml`. The identity and the database stay in
     * `noBackupFilesDir` regardless, because those must never be restored onto
     * a second device.
     */
    private val syncDir = File(appContext.filesDir, "sync")

    /** Where it used to be, for installs that predate the move. */
    private val formerSyncDir = File(appContext.noBackupFilesDir, "sync")

    private val wifiManager: WifiManager? =
        appContext.getSystemService(WifiManager::class.java)

    private var multicastLock: WifiManager.MulticastLock? = null

    /**
     * Created on first use and never replaced.
     *
     * The engine holds the database, the block store and the identity, so a
     * second instance over the same directory would be a second writer. One per
     * process, for the life of the process.
     */
    val engine: Engine by lazy {
        moveAnyFilesLeftInTheOldPlace()
        Engine(baseDir.absolutePath, syncDir.absolutePath).also(::nameOnFirstRun)
    }

    /**
     * Carries files over from the directory this app used to use.
     *
     * Before the engine is constructed, so the catalog it reads and the files
     * on disk describe the same thing. The catalog stores paths relative to the
     * sync folder, so moving the folder is invisible to it - but only if the
     * files arrive before it looks.
     */
    private fun moveAnyFilesLeftInTheOldPlace() {
        if (!formerSyncDir.isDirectory) return
        runCatching {
            syncDir.mkdirs()
            formerSyncDir.listFiles()?.forEach { file ->
                val destination = File(syncDir, file.name)
                if (!destination.exists()) file.renameTo(destination)
            }
            formerSyncDir.delete()
        }
    }

    /**
     * Tells the engine what this phone is called, once.
     *
     * The engine has to guess a name from inside Rust, and on Android there is
     * nothing good to guess from - `whoami` reports "Unknown". Android does know:
     * the user may have named the device in Settings, and failing that there is
     * always a model. So the platform supplies it.
     *
     * Once, and only once. After the first run the name belongs to whoever last
     * set it, and re-applying the model on every launch would quietly undo a
     * rename every time the app restarted.
     */
    private fun nameOnFirstRun(engine: Engine) {
        if (preferences.deviceHasBeenNamed) return
        runCatching { engine.setDeviceName(platformDeviceName()) }
            .onSuccess { preferences.deviceHasBeenNamed = true }
    }

    /**
     * The best name Android can offer: the one the owner chose, or the model.
     */
    private fun platformDeviceName(): String {
        val chosen = Settings.Global.getString(
            appContext.contentResolver,
            Settings.Global.DEVICE_NAME,
        )
        if (!chosen.isNullOrBlank()) return chosen.trim()

        val manufacturer = Build.MANUFACTURER.orEmpty().trim()
        val model = Build.MODEL.orEmpty().trim()
        return when {
            model.isBlank() -> "Android device"
            manufacturer.isBlank() -> model
            // "Google Pixel 7", but not "Samsung Samsung Galaxy S24".
            model.startsWith(manufacturer, ignoreCase = true) -> model
            else -> "$manufacturer $model"
        }
    }

    val syncDirPath: String get() = syncDir.absolutePath

    fun setEventListener(listener: EventListener) {
        engine.setEventListener(listener)
    }

    /**
     * Starts discovery, the server and replication.
     *
     * The lock is taken before the engine rather than after: mDNS announces
     * itself and starts listening as part of starting, and anything that
     * arrives before the lock is held is simply not received.
     */
    fun start() {
        acquireMulticastLock()
        engine.start()
    }

    /**
     * Stops everything and gives the multicast lock back.
     *
     * Releasing matters. A held multicast lock keeps the Wi-Fi chip taking
     * every multicast frame on the network, which is a real and continuous
     * drain, and this app has no business doing that while it is not on screen.
     */
    fun stop() {
        engine.stop()
        releaseMulticastLock()
    }

    private fun acquireMulticastLock() {
        if (multicastLock?.isHeld == true) return
        multicastLock = wifiManager?.createMulticastLock(MULTICAST_LOCK_TAG)?.apply {
            // Not reference counted: acquire and release are driven by the
            // process moving between foreground and background, which is a
            // state rather than a count, and a mismatched pair would either
            // leak the lock or throw.
            setReferenceCounted(false)
            acquire()
        }
    }

    private fun releaseMulticastLock() {
        multicastLock?.takeIf { it.isHeld }?.release()
        multicastLock = null
    }

    private companion object {
        const val MULTICAST_LOCK_TAG = "localcloud-mdns"
    }
}
