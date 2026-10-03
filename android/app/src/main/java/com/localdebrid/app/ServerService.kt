package com.localdebrid.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors

/**
 * Keeps the server alive while the phone is locked: foreground service, partial wake
 * lock (CPU) and Wi-Fi lock. The server itself runs in-process through JNI.
 */
class ServerService : Service() {
    companion object {
        private const val TAG = "LocalDebrid"
        private const val ACTION_START = "com.localdebrid.app.START"
        private const val ACTION_STOP = "com.localdebrid.app.STOP"
        private const val CHANNEL = "server"
        private const val NOTIFICATION_ID = 1
        private const val POLL_MS = 3000L

        fun start(context: Context) {
            val intent = Intent(context, ServerService::class.java).setAction(ACTION_START)
            context.startForegroundService(intent)
        }

        fun stop(context: Context) {
            val intent = Intent(context, ServerService::class.java).setAction(ACTION_STOP)
            context.startService(intent)
        }

        fun dataDir(context: Context) = File(context.noBackupFilesDir, "server")
        fun cacheDir(context: Context) = File(context.noBackupFilesDir, "torrents")
    }

    private val lifecycle = Executors.newSingleThreadExecutor()
    private val poller = Executors.newSingleThreadScheduledExecutor()
    private val main = Handler(Looper.getMainLooper())
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLocks: List<WifiManager.WifiLock> = emptyList()
    private var active = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val channel = NotificationChannel(CHANNEL, getString(R.string.notification_channel), NotificationManager.IMPORTANCE_LOW)
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        poller.scheduleWithFixedDelay({ poll() }, POLL_MS, POLL_MS, java.util.concurrent.TimeUnit.MILLISECONDS)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // A null intent means the system restarted us after killing the process.
        when (intent?.action ?: ACTION_START) {
            ACTION_STOP -> stopServer()
            else -> startServer()
        }
        return START_STICKY
    }

    private fun startServer() {
        enterForeground(getString(R.string.status_starting), null)
        if (active) return
        active = true
        acquireLocks()
        ServerController.update(ServerController.Snapshot(ServerController.Phase.STARTING))
        lifecycle.execute {
            val error = try {
                NativeServer.nativeInit(applicationContext)
                NativeServer.nativeStart(dataDir(this).absolutePath, cacheDir(this).absolutePath)
            } catch (t: Throwable) {
                Log.e(TAG, "native start failed", t)
                t.message ?: t.toString()
            }
            if (error.isEmpty()) {
                poll()
            } else {
                ServerController.update(ServerController.Snapshot(ServerController.Phase.FAILED, error = error))
                main.post { shutdown(keepFailure = true) }
            }
        }
    }

    private fun stopServer() {
        if (!active) {
            shutdown(keepFailure = false)
            return
        }
        ServerController.update(ServerController.Snapshot(ServerController.Phase.STOPPING))
        lifecycle.execute {
            NativeServer.nativeStop()
            main.post { shutdown(keepFailure = false) }
        }
    }

    private fun shutdown(keepFailure: Boolean) {
        active = false
        releaseLocks()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
        if (!keepFailure) ServerController.update(ServerController.Snapshot(ServerController.Phase.STOPPED))
    }

    private fun poll() {
        if (!active) return
        val status = try {
            JSONObject(NativeServer.nativeStatus())
        } catch (t: Throwable) {
            return
        }
        if (!status.optBoolean("running")) {
            if (status.has("error")) {
                ServerController.update(ServerController.Snapshot(ServerController.Phase.FAILED, status, status.optString("error")))
            }
            return
        }
        ServerController.update(ServerController.Snapshot(ServerController.Phase.RUNNING, status))
        val urls = status.optJSONArray("urls")
        val ip = urls?.optJSONObject(0)?.optString("ip") ?: "?"
        val engine = status.optJSONObject("engine")
        val detail = getString(
            R.string.notification_detail,
            formatSpeed(engine?.optLong("downloadBps") ?: 0),
            engine?.optInt("peersLive") ?: 0,
            status.optInt("sessions"),
        )
        main.post { if (active) enterForeground("$ip:${status.optInt("port")}", detail) }
    }

    private fun enterForeground(title: String, text: String?) {
        val openApp = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(
            this, 1, Intent(this, ServerService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_server)
            .setContentTitle(getString(R.string.notification_title, title))
            .setContentText(text)
            .setContentIntent(openApp)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .addAction(Notification.Action.Builder(null, getString(R.string.stop_server), stop).build())
            .build()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    @Suppress("DEPRECATION")
    private fun acquireLocks() {
        val power = getSystemService(PowerManager::class.java)
        wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "LocalDebrid:server").apply {
            setReferenceCounted(false)
            acquire()
        }
        val wifi = applicationContext.getSystemService(WifiManager::class.java)
        val modes = buildList {
            // HIGH_PERF is a no-op from API 34; LOW_LATENCY only applies with the screen on.
            add(WifiManager.WIFI_MODE_FULL_HIGH_PERF)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) add(WifiManager.WIFI_MODE_FULL_LOW_LATENCY)
        }
        wifiLocks = modes.map { mode ->
            wifi.createWifiLock(mode, "LocalDebrid:wifi:$mode").apply {
                setReferenceCounted(false)
                acquire()
            }
        }
    }

    private fun releaseLocks() {
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        wifiLocks.forEach { if (it.isHeld) it.release() }
        wifiLocks = emptyList()
    }

    override fun onDestroy() {
        poller.shutdownNow()
        if (active) {
            active = false
            Thread { NativeServer.nativeStop() }.start()
        }
        releaseLocks()
        super.onDestroy()
    }
}
