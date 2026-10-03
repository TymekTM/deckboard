//! Foreground keep-alive service (ROADMAP M4): without it Android's
//! Doze starves the WebSocket the moment the screen goes dark, and the
//! deck wakes up disconnected. The socket itself stays in the
//! ViewModel; the service only raises the process to foreground
//! priority, flips LinkBus.keepLinkInBackground (so background no
//! longer closes the link - plan 008's close remains the fallback),
//! and mirrors the link line into a low-priority notification whose
//! "Rozłącz" action puts the old battery behavior back.

package app.pulpit.mobile

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import app.pulpit.mobile.state.LinkBus
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

class LinkService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val notification = buildNotification(LinkBus.status.value)
        // dataSync type is enforced on API 34+; ServiceCompat no-ops the
        // typed call on the deck's older Android.
        ServiceCompat.startForeground(
            this,
            NOTIFICATION_ID,
            notification,
            ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
        )
        scope.launch {
            LinkBus.status.collect { text ->
                val nm = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
                nm.notify(NOTIFICATION_ID, buildNotification(text))
            }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                LinkBus.keepLinkInBackground.value = false
                stopSelf()
            }
            else -> LinkBus.keepLinkInBackground.value = true
        }
        return START_STICKY
    }

    override fun onDestroy() {
        LinkBus.keepLinkInBackground.value = false
        scope.cancel()
        super.onDestroy()
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        // Swiping the app away destroys the ViewModel and its socket with
        // it; a "connected" notification would be a lie from then on.
        LinkBus.keepLinkInBackground.value = false
        stopSelf()
        super.onTaskRemoved(rootIntent)
    }

    private fun buildNotification(text: String): Notification {
        if (Build.VERSION.SDK_INT >= 26) {
            val ch = NotificationChannel(
                CHANNEL_ID,
                "Łącze z Pulpitem",
                NotificationManager.IMPORTANCE_LOW,
            )
            (getSystemService(NOTIFICATION_SERVICE) as NotificationManager)
                .createNotificationChannel(ch)
        }
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, LinkService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val builder = if (Build.VERSION.SDK_INT >= 26) {
            NotificationCompat.Builder(this, CHANNEL_ID)
        } else {
            @Suppress("DEPRECATION")
            NotificationCompat.Builder(this)
        }
        return builder
            .setSmallIcon(R.drawable.ic_launcher)
            .setContentTitle("Pulpit")
            .setContentText(text.ifBlank { "utrzymuje łącze z serwerem" })
            .setOngoing(true)
            .setContentIntent(open)
            .addAction(0, "Rozłącz", stop)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    companion object {
        const val CHANNEL_ID = "link"
        const val ACTION_STOP = "app.pulpit.mobile.link.STOP"
        const val NOTIFICATION_ID = 42
    }
}
