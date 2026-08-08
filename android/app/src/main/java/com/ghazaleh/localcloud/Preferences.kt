package com.ghazaleh.localcloud

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The few things this app remembers that the engine does not.
 *
 * Deliberately small, and deliberately not in the engine's database. Whether
 * this phone should keep syncing with the screen off is a fact about this
 * installation, not about the mesh - no other device has an opinion on it and
 * none should ever be told.
 */
class Preferences(context: Context) {

    private val store = context.applicationContext
        .getSharedPreferences("localcloud", Context.MODE_PRIVATE)

    private val _backgroundSync = MutableStateFlow(store.getBoolean(KEY_BACKGROUND_SYNC, false))

    /**
     * Whether the foreground service should be running.
     *
     * The setting is the source of truth rather than the service itself: a
     * service can be killed for memory and restarted by the system, and a
     * switch that flickered with it would be reporting the wrong thing. This
     * says what the person asked for, and the service is kept to match.
     *
     * Off by default. Syncing with the screen off is a real cost in battery and
     * a persistent notification, and is not something to assume.
     */
    val backgroundSync: StateFlow<Boolean> = _backgroundSync.asStateFlow()

    fun setBackgroundSync(enabled: Boolean) {
        store.edit().putBoolean(KEY_BACKGROUND_SYNC, enabled).apply()
        _backgroundSync.value = enabled
    }

    /**
     * Whether the engine has ever been handed the name Android knows.
     *
     * Once true it stays true, so a later rename is never overwritten by the
     * model name at the next launch.
     */
    var deviceHasBeenNamed: Boolean
        get() = store.getBoolean(KEY_DEVICE_NAMED, false)
        set(value) {
            store.edit().putBoolean(KEY_DEVICE_NAMED, value).apply()
        }

    private companion object {
        const val KEY_BACKGROUND_SYNC = "background-sync"
        const val KEY_DEVICE_NAMED = "device-named"
    }
}
