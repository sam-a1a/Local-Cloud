package com.ghazaleh.localcloud.service

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import android.util.Log
import com.ghazaleh.localcloud.LocalCloudApplication
import com.ghazaleh.localcloud.engine.EngineRepository

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
    }

    /**
     * `START_STICKY` so that a service killed for memory comes back.
     *
     * The intent is not redelivered and does not need to be: being started is
     * the entire instruction, and [onCreate] carries it out. Explicitly
     * stopping the service is still final - the system does not restart what a
     * person switched off.
     */
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
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

        private const val TAG = "SyncService"
    }
}
