package com.ghazaleh.localcloud.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import com.ghazaleh.localcloud.MainActivity
import com.ghazaleh.localcloud.R

/**
 * The notification that has to exist for the engine to keep running.
 *
 * Not a nicety and not an announcement: Android will not let a process do
 * network work with no screen unless it is showing one of these, and it is the
 * right trade. A device that quietly stays on a mesh, holds a multicast lock and
 * accepts files while the app looks closed should say so, somewhere the person
 * can see it and switch it off.
 *
 * Kept deliberately quiet - [NotificationManager.IMPORTANCE_LOW], no badge, no
 * sound. It is a status light, not an interruption.
 */
object SyncNotification {

    const val CHANNEL_ID = "background-sync"

    /** Stable, because updating the notification means posting this id again. */
    const val ID = 1

    fun ensureChannel(context: Context) {
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Background syncing",
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description =
                "Shown while LocalCloud is keeping this device on the mesh with the app closed."
            setShowBadge(false)
        }
        context.getSystemService(NotificationManager::class.java)
            .createNotificationChannel(channel)
    }

    /**
     * @param text one line describing what the engine is doing right now.
     */
    fun build(context: Context, text: String): Notification =
        NotificationCompat.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_localcloud)
            .setContentTitle("LocalCloud")
            .setContentText(text)
            .setContentIntent(openTheApp(context))
            // Ongoing and low priority: it cannot be swiped away while the
            // service runs, and it sorts below anything that wants attention.
            .setOngoing(true)
            .setSilent(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setShowWhen(false)
            .build()

    private fun openTheApp(context: Context): PendingIntent {
        val intent = Intent(context, MainActivity::class.java)
            .setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        return PendingIntent.getActivity(
            context,
            0,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }
}
