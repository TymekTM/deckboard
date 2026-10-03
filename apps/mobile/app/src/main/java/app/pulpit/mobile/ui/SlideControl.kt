//! Shared drag protocol for slider and knob tiles (round 4, MOB-13):
//! both widgets used to carry a copy-pasted block of drag state - the
//! remembered drag value, the live-echo policy, throttled sends, and
//! the converge-on-release guarantee. One controller owns them now, so
//! a fix to the protocol (throttle tuning, gesture interplay) lands
//! once for both templates.

package app.pulpit.mobile.ui

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/** Slide send cadence: the desktop's touch mode sends at most ~7/s
 *  (SLIDER_SEND_INTERVAL = 150 in TileCell.vue); 120 ms puts the deck
 *  in the same order without feeling laggy (round 4, MOB-11 - this
 *  used to be 30 ms, 5x the desktop's rate of Wi-Fi wakeups and
 *  server execs). */
const val SLIDE_THROTTLE_MS = 120L

/** Drag events fire hundreds of times per gesture and every slide send is
 *  a websocket round-trip the server executes - ship at most one value
 *  per [throttleMs], plus the final one on release ([push] with
 *  `force = true`). One instance per tile, remembered alongside it. */
class SlideThrottle(private val throttleMs: Long = SLIDE_THROTTLE_MS) {
    private var lastSentAt = 0L

    fun push(value: Float, force: Boolean = false, send: (Float) -> Unit) {
        val now = System.currentTimeMillis()
        if (force || now - lastSentAt >= throttleMs) {
            lastSentAt = now
            send(value)
        }
    }
}

/** Drag state shared by [SliderTile] (Tile.kt) and [KnobTile]
 *  (Widgets.kt), one instance per tile id:
 *  - null [dragValue] until somebody drags: the channel's live value
 *    drives the widget then (the desktop's touch mode mirrors the same
 *    way);
 *  - a completed drag keeps its position - the face must not flicker
 *    back while the echo patch travels - a cancelled one hands control
 *    back to live;
 *  - every send is throttled except the forced final one, so release
 *    and cancel always converge on the last sampled value. */
class SlideDragController(private val throttle: SlideThrottle = SlideThrottle()) {

    var dragValue: Float? by mutableStateOf(null)
        private set

    /** The value to render: the drag's own while it lasts, else live. */
    fun current(live: Double?): Float =
        dragValue ?: live?.coerceIn(0.0, 1.0)?.toFloat() ?: 0.5f

    /** The drag begins at [v] (e.g. where the finger landed) - always
     *  sent, so the first positioning is not at the throttle's mercy. */
    fun start(v: Float, send: (Float) -> Unit) {
        dragValue = v
        throttle.push(v, force = true, send = send)
    }

    fun move(v: Float, send: (Float) -> Unit) {
        dragValue = v
        throttle.push(v, send = send)
    }

    /** Release: converge - the last sampled value always reaches the
     *  server, throttling only smooths the path. */
    fun end(live: Double?, send: (Float) -> Unit) {
        throttle.push(current(live), force = true, send = send)
    }

    /** A cancelled drag still commits its last sampled position (like
     *  the desktop), then follows live again. */
    fun cancel(live: Double?, send: (Float) -> Unit) {
        end(live, send)
        dragValue = null
    }
}
