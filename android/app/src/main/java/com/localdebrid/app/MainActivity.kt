package com.localdebrid.app

import android.Manifest
import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import android.view.View
import android.widget.Button
import android.widget.TextView
import android.widget.Toast
import org.json.JSONObject

class MainActivity : Activity() {
    private lateinit var statusText: TextView
    private lateinit var httpsUrl: TextView
    private lateinit var httpUrl: TextView
    private lateinit var stats: TextView
    private lateinit var warning: TextView
    private lateinit var toggle: Button
    private lateinit var battery: Button
    private lateinit var dashboard: Button
    private lateinit var urlsBlock: View
    private val listener: (ServerController.Snapshot) -> Unit = { render(it) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        applySystemBarPadding(findViewById(R.id.root))
        statusText = findViewById(R.id.status)
        httpsUrl = findViewById(R.id.https_url)
        httpUrl = findViewById(R.id.http_url)
        stats = findViewById(R.id.stats)
        warning = findViewById(R.id.warning)
        toggle = findViewById(R.id.toggle)
        battery = findViewById(R.id.battery)
        dashboard = findViewById(R.id.dashboard)
        urlsBlock = findViewById(R.id.urls)

        toggle.setOnClickListener {
            when (ServerController.snapshot.phase) {
                ServerController.Phase.RUNNING, ServerController.Phase.STARTING -> ServerService.stop(this)
                else -> {
                    requestNotificationPermission()
                    ServerService.start(this)
                }
            }
        }
        battery.setOnClickListener { requestBatteryExemption() }
        dashboard.setOnClickListener { startActivity(Intent(this, DashboardActivity::class.java)) }
        findViewById<Button>(R.id.copy_https).setOnClickListener { copy(httpsUrl.text) }
        findViewById<Button>(R.id.copy_http).setOnClickListener { copy(httpUrl.text) }
    }

    override fun onStart() {
        super.onStart()
        ServerController.observe(listener)
        updateBatteryButton()
    }

    override fun onStop() {
        ServerController.remove(listener)
        super.onStop()
    }

    private fun render(snapshot: ServerController.Snapshot) {
        val status = snapshot.status
        statusText.text = when (snapshot.phase) {
            ServerController.Phase.STOPPED -> getString(R.string.status_stopped)
            ServerController.Phase.STARTING -> getString(R.string.status_starting)
            ServerController.Phase.RUNNING -> getString(R.string.status_running)
            ServerController.Phase.STOPPING -> getString(R.string.status_stopping)
            ServerController.Phase.FAILED -> getString(R.string.status_failed, snapshot.error ?: "")
        }
        toggle.text = getString(
            if (snapshot.phase == ServerController.Phase.RUNNING || snapshot.phase == ServerController.Phase.STARTING) {
                R.string.stop_server
            } else {
                R.string.start_server
            },
        )
        toggle.isEnabled = snapshot.phase != ServerController.Phase.STOPPING
        val running = snapshot.phase == ServerController.Phase.RUNNING && status != null
        urlsBlock.visibility = if (running) View.VISIBLE else View.GONE
        dashboard.isEnabled = running
        stats.visibility = if (running) View.VISIBLE else View.GONE
        if (running) renderStatus(status!!)
        renderWarnings(status)
    }

    private fun renderStatus(status: JSONObject) {
        val first = status.optJSONArray("urls")?.optJSONObject(0)
        httpsUrl.text = first?.optString("httpsManifest")?.takeIf { it.isNotEmpty() && it != "null" }
            ?: getString(R.string.https_unavailable)
        httpUrl.text = first?.optString("httpManifest") ?: "—"
        val engine = status.optJSONObject("engine")
        stats.text = getString(
            R.string.stats,
            status.optInt("sessions"),
            status.optInt("activeStreams"),
            formatSpeed(engine?.optLong("downloadBps") ?: 0),
            formatSpeed(engine?.optLong("uploadBps") ?: 0),
            engine?.optInt("peersLive") ?: 0,
            formatBytes(status.optLong("cacheUsedBytes")),
            formatBytes(status.optLong("freeDiskBytes")),
        )
    }

    private fun renderWarnings(status: JSONObject?) {
        val messages = mutableListOf<String>()
        if (!onWifi()) messages += getString(R.string.warning_no_wifi)
        val tls = status?.optJSONObject("tls")
        if (tls != null && tls.optString("mode") != "Off" && !tls.optBoolean("ready")) {
            messages += getString(R.string.warning_tls, tls.optString("error"))
        }
        if (status?.optBoolean("restartPending") == true) messages += getString(R.string.warning_restart)
        warning.text = messages.joinToString("\n\n")
        warning.visibility = if (messages.isEmpty()) View.GONE else View.VISIBLE
    }

    private fun onWifi(): Boolean {
        val cm = getSystemService(ConnectivityManager::class.java)
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) ||
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)
    }

    private fun copy(text: CharSequence) {
        getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("Local Debrid", text))
        Toast.makeText(this, R.string.copied, Toast.LENGTH_SHORT).show()
    }

    private fun updateBatteryButton() {
        val power = getSystemService(PowerManager::class.java)
        battery.visibility = if (power.isIgnoringBatteryOptimizations(packageName)) View.GONE else View.VISIBLE
    }

    @Suppress("BatteryLife")
    private fun requestBatteryExemption() {
        val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName"))
        try {
            startActivity(intent)
        } catch (e: Exception) {
            startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
        }
    }

    private fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
    }
}
