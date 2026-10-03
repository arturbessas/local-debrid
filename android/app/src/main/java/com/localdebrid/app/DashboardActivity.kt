package com.localdebrid.app

import android.annotation.SuppressLint
import android.app.Activity
import android.os.Bundle
import android.webkit.WebView
import android.webkit.WebViewClient

/** Shows the server's own web dashboard (sessions, cache, settings, logs). */
class DashboardActivity : Activity() {
    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val web = WebView(this)
        web.settings.javaScriptEnabled = true
        web.settings.domStorageEnabled = true
        web.webViewClient = WebViewClient()
        setContentView(web)
        applySystemBarPadding(web)
        val url = ServerController.snapshot.status?.optString("localDashboard")
        if (url.isNullOrEmpty()) finish() else web.loadUrl(url)
    }
}
