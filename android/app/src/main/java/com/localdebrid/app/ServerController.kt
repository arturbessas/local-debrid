package com.localdebrid.app

import android.os.Handler
import android.os.Looper
import org.json.JSONObject
import java.util.concurrent.CopyOnWriteArrayList

/** Process-wide server state shared by the service and the UI. */
object ServerController {
    enum class Phase { STOPPED, STARTING, RUNNING, STOPPING, FAILED }

    data class Snapshot(val phase: Phase, val status: JSONObject? = null, val error: String? = null)

    @Volatile
    var snapshot = Snapshot(Phase.STOPPED)
        private set

    private val listeners = CopyOnWriteArrayList<(Snapshot) -> Unit>()
    private val main = Handler(Looper.getMainLooper())

    fun update(next: Snapshot) {
        snapshot = next
        main.post { listeners.forEach { it(next) } }
    }

    fun observe(listener: (Snapshot) -> Unit) {
        listeners.add(listener)
        listener(snapshot)
    }

    fun remove(listener: (Snapshot) -> Unit) {
        listeners.remove(listener)
    }
}
