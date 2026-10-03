//! Pure gesture classification for M5 custom gestures: the completed
//! drag of a swipe becomes a named interaction. Unit-tested - the
//! Compose-side detection only feeds it coordinates.

package app.pulpit.mobile.ui

/** Swipes the deck recognizes; up/down stay unrecognized on purpose
 *  (boards scroll vertically in no direction - a vertical flick is far
 *  likelier a sloppy tap than intent). */
enum class Swipe {
    None,
    Left,
    Right,
}

/** Dominant-axis classification of a completed drag. A swipe must
 *  travel at least [minPx] AND be clearly horizontal (|dx| > |dy|);
 *  anything else reads as None, never as a stray trigger. */
fun classifySwipe(dx: Float, dy: Float, minPx: Float): Swipe {
    val ax = kotlin.math.abs(dx)
    val ay = kotlin.math.abs(dy)
    if (ax < minPx || ax <= ay) return Swipe.None
    return if (dx < 0) Swipe.Left else Swipe.Right
}

/** Shared per-tile gesture state: the swipe observer and the tap
 *  detector live in separate pointerInput blocks, and the tap path must
 *  know when one of them already consumed the touch (a long press or a
 *  swipe must not ALSO fire the release tap - one touch, exactly one
 *  interaction). */
class GestureTracker {
    var longPressed: Boolean = false
    var doubleTapped: Boolean = false
    var moved: Boolean = false

    fun reset() {
        longPressed = false
        doubleTapped = false
        moved = false
    }

    /** True when the touch is already spoken for. */
    val consumed: Boolean
        get() = longPressed || doubleTapped || moved
}

/** Minimum horizontal travel for a flick to count as a swipe; tiles are
 *  ~150 px on the deck, so 64 px is a deliberate move, not a wiggle. */
const val SWIPE_MIN_PX = 64f
