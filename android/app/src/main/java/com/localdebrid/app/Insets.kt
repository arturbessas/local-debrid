package com.localdebrid.app

import android.os.Build
import android.view.View
import android.view.WindowInsets

/**
 * Pads [view] by the system bars and display cutout. From targetSdk 35 Android draws
 * every app edge-to-edge, so without this the content sits under the status bar.
 */
fun applySystemBarPadding(view: View) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return
    val base = intArrayOf(view.paddingLeft, view.paddingTop, view.paddingRight, view.paddingBottom)
    view.setOnApplyWindowInsetsListener { v, insets ->
        val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
        v.setPadding(base[0] + bars.left, base[1] + bars.top, base[2] + bars.right, base[3] + bars.bottom)
        insets
    }
    view.requestApplyInsets()
}
