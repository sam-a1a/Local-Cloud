package com.ghazaleh.localcloud.service

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import com.ghazaleh.localcloud.LocalCloudApplication
import com.ghazaleh.localcloud.engine.EngineRepository
import com.ghazaleh.localcloud.engine.MeshState
import com.ghazaleh.localcloud.engine.Transfer
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.flow.onEach

/**
 * Keeps this device on the mesh while the app is closed.
 *
 * The service does almost nothing itself. The engine already lives in the
 * process and already knows how to run; all this adds is a second reason for it
 * to keep running once every screen has gone, and the ongoing notification that
 * Android requires in exchange.
 *
 * It is not bound and has no interface. Nothing needs to talk to it - starting
 * it is the whole instruction, and stopping it is the other one.
 */
class SyncService : Service() {

    /**
     * Lives exactly as long as the service.
     *
     * Only the notification is driven from here, and a notification for a
     * service that has stopped is worse than none - so this is cancelled in
     * [onDestroy], unlike the engine work, which deliberately outlives it.
     */
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    override fun onCreate() {
        super.onCreate()

        // Before anything else. Android gives a service a few seconds to go
        // foreground after being started and kills it if it does not, so this
        // must not wait behind engine work.
        startForeground(
            SyncNotification.ID,
            SyncNotification.build(this, "Keeping this device on the mesh"),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
        )

        app().engineNeededBy(EngineRepository.RunReason.BackgroundService)
        keepTheNotificationTrue()
    }

    /**
     * `START_STICKY` so that a service killed for memory comes back.
     *
     * The intent is not redelivered and does not need to be: being started is
     * the entire instruction, and [onCreate] carries it out. Explicitly
     * stopping the service is still final - the system does not restart what a
     * person switched off.
     */
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            // The setting, not just the service. Stopping only the service
            // would leave a switch claiming this is on, and the next time the
            // app opened it would start again.
            app().preferences.setBackgroundSync(false)
            stopSelf()
            return START_NOT_STICKY
        }
        return START_STICKY
    }

    /**
     * Rewrites the notification whenever what it says stops being true.
     *
     * Distinct values only, so a hundred block-progress events during one
     * transfer post one notification rather than a hundred.
     */
    private fun keepTheNotificationTrue() {
        val repository = app().repository
        combine(repository.state, repository.transfers, ::summarise)
            .distinctUntilChanged()
            .onEach { line ->
                val manager = NotificationManagerCompat.from(this)
                if (manager.areNotificationsEnabled()) {
                    runCatching { manager.notify(SyncNotification.ID, SyncNotification.build(this, line)) }
                }
            }
            .launchIn(scope)
    }

    override fun onDestroy() {
        scope.cancel()
        // Handed to the application's scope rather than one of the service's
        // own: this call outlives the service by design, and a scope cancelled
        // in `onDestroy` would cancel the very work that releases the engine.
        app().engineNoLongerNeededBy(EngineRepository.RunReason.BackgroundService)
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun app() = application as LocalCloudApplication

    companion object {

        /**
         * Starts it, if Android will allow it.
         *
         * A foreground service may only be started while the app is visible, so
         * the caller is expected to be a tap. It is still guarded: the rules
         * differ by version and by how the app came to be in the foreground,
         * and the failure should be a message rather than a crash.
         */
        fun start(context: Context): Boolean = runCatching {
            context.startForegroundService(Intent(context, SyncService::class.java))
        }.onFailure {
            Log.w(TAG, "Could not start background syncing", it)
        }.isSuccess

        fun stop(context: Context) {
            context.stopService(Intent(context, SyncService::class.java))
        }

        const val ACTION_STOP = "com.ghazaleh.localcloud.STOP_BACKGROUND_SYNC"

        private const val TAG = "SyncService"
    }
}

/**
 * One line describing what the engine is doing, for the notification.
 *
 * Deliberately about the mesh rather than about the app. "On the mesh with 2
 * devices" is a reason for this to be running; "Running" is not.
 */
private fun summarise(state: MeshState, transfers: Map<String, Transfer>): String {
    val sending = transfers.values.count { it.direction == Transfer.Direction.Sending }
    val receiving = transfers.values.count { it.direction == Transfer.Direction.Receiving }
    val nearby = state.reachable.size

    return when {
        !state.running -> "Starting…"
        sending > 0 && receiving > 0 -> "Sending $sending, receiving $receiving"
        sending > 0 -> "Sending ${sending.files()}"
        receiving > 0 -> "Receiving ${receiving.files()}"
        state.paired.isEmpty() -> "Waiting to be paired with another device"
        nearby > 0 -> "On the mesh with ${nearby.devices()}"
        else -> "No paired devices on this network"
    }
}

private fun Int.files(): String = if (this == 1) "1 file" else "$this files"

private fun Int.devices(): String = if (this == 1) "1 device" else "$this devices"
