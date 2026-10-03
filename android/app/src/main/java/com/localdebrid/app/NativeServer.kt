package com.localdebrid.app

import android.content.Context

/** JNI bridge to the Rust server (crates/localdebrid-jni). */
object NativeServer {
    init {
        System.loadLibrary("localdebrid")
    }

    @JvmStatic external fun nativeInit(context: Context)

    /** Blocks until the server listens. Returns "" on success or the error message. */
    @JvmStatic external fun nativeStart(dataDir: String, cacheDir: String): String

    @JvmStatic external fun nativeStop()

    /** JSON, see `Status` in crates/localdebrid/src/app.rs plus `running` and `localDashboard`. */
    @JvmStatic external fun nativeStatus(): String
}
