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

/** Slider step haptic rate-limiter: emits tactile ticks when the drag
 *  crosses a discrete step (~5% by default), rate-limited so rapid flicks
 *  do not buzz continuously. Zero allocations per frame. Pure clock for
 *  deterministic unit testing. */
class SliderStepLimiter(
    val step: Float = DEFAULT_STEP,
    val rateLimitMs: Long = DEFAULT_RATE_LIMIT_MS,
    private val clock: () -> Long = System::currentTimeMillis,
) {
    companion object {
        const val DEFAULT_STEP = 0.05f
        const val DEFAULT_RATE_LIMIT_MS = 50L
    }

    private var lastStepIndex: Int? = null
    private var lastTickTimeMs: Long = 0L

    fun reset(initialValue: Float? = null) {
        lastStepIndex = initialValue?.let { stepIndex(it) }
        lastTickTimeMs = 0L
    }

    fun stepIndex(value: Float): Int {
        val clamped = value.coerceIn(0f, 1f)
        return (clamped / step).toInt()
    }

    /** Returns true if a step boundary was crossed and rate limit permits. */
    fun onMove(value: Float): Boolean {
        val currentIndex = stepIndex(value)
        val previousIndex = lastStepIndex

        if (previousIndex == null) {
            lastStepIndex = currentIndex
            return false
        }

        if (currentIndex != previousIndex) {
            val now = clock()
            if (now - lastTickTimeMs >= rateLimitMs) {
                lastTickTimeMs = now
                lastStepIndex = currentIndex
                return true
            }
        }
        return false
    }
}

/** Drag state shared by [SliderTile] (Tile.kt) and [KnobTile]
 *  (Widgets.kt), one instance per tile id:
 *  - null [dragValue] until somebody drags: the channel's live value
 *    drives the widget then (the desktop's touch mode mirrors the same
 *    way);
 *  - a completed drag keeps its position only until the channel's live
 *    value moves off what it was at release: the face must not flicker
 *    back while the echo patch travels, but once the server reports a
 *    new value (the echo, or a change made elsewhere) the widget follows
 *    live again - [onLive] drops the stale drag position. A cancelled
 *    drag hands control back to live at once;
 *  - every send is throttled except the forced final one, so release
 *    and cancel always converge on the last sampled value;
 *  - step-limited haptic feedback fires light ticks during drag. */
class SlideDragController(
    private val throttle: SlideThrottle = SlideThrottle(),
    val stepLimiter: SliderStepLimiter = SliderStepLimiter(),
) {

    var dragValue: Float? by mutableStateOf(null)
        private set

    private var dragging by mutableStateOf(false)

    /** The channel's live value when the finger lifted; the drag's
     *  position is held for as long as live still reads this. */
    private var liveAtRelease: Double? by mutableStateOf(null)

    /** The value to render: the drag's own while it lasts (and until
     *  live moves after release), else live. */
    fun current(live: Double?): Float {
        val held = dragValue
        if (held != null && (dragging || live == liveAtRelease)) return held
        return live?.coerceIn(0.0, 1.0)?.toFloat() ?: held ?: 0.5f
    }

    /** Call when the live value changes (a LaunchedEffect keyed on it):
     *  once a released drag is no longer what live reports, forget the
     *  drag position so the widget tracks the server from here on. */
    fun onLive(live: Double?) {
        if (!dragging && dragValue != null && live != liveAtRelease) {
            dragValue = null
        }
    }

    /** The drag begins at [v] (e.g. where the finger landed) - always
     *  sent, so the first positioning is not at the throttle's mercy. */
    fun start(v: Float, send: (Float) -> Unit) {
        dragging = true
        dragValue = v
        stepLimiter.reset(v)
        throttle.push(v, force = true, send = send)
    }

    fun move(v: Float, onStepTick: () -> Unit, send: (Float) -> Unit) {
        dragValue = v
        if (stepLimiter.onMove(v)) {
            onStepTick()
        }
        throttle.push(v, send = send)
    }

    fun move(v: Float, send: (Float) -> Unit) {
        move(v, onStepTick = {}, send = send)
    }

    /** Release: converge - the last sampled value always reaches the
     *  server, throttling only smooths the path. */
    fun end(live: Double?, send: (Float) -> Unit) {
        throttle.push(current(live), force = true, send = send)
        dragging = false
        liveAtRelease = live
    }

    /** A cancelled drag still commits its last sampled position (like
     *  the desktop), then follows live again. */
    fun cancel(live: Double?, send: (Float) -> Unit) {
        end(live, send)
        dragValue = null
    }
}