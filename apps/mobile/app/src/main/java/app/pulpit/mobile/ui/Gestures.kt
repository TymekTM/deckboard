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

/** What the tap detector's release (Tile.kt onPress finally) owes the
 *  server. Fire = the release callback (press-end, or the tap);
 *  Cancel = reset the face without sending anything. */
enum class ReleaseDecision { Fire, Cancel }

/** The release decision, pure for GesturesTest (round 4):
 *  - [consumed]: a custom gesture (long-press, swipe, double-tap)
 *    already sent this touch's interaction - the plain tap must not
 *    double it. But a tile that sent press-start still owes press-end
 *    (MOB-02): only press-end stops the server's hold-repeat loop and
 *    releases a held key, so a consumed gesture on a press-mode tile
 *    fires too - the extra release-phase exec is harmless server-side.
 *  - [released]: tryAwaitRelease returned true (finger lifted in
 *    place); false = the touch was stolen and only a press-mode tile
 *    still fires (the D1 phantom-tap rule).
 *  - [declaresPressPair]: the tile declares press-start AND press-end.
 *  - [tapSentFromOnTap]: the tap rides detectTapGestures' onTap
 *    instead (double-tap tiles, MOB-03), so the release path never
 *    fires. */
fun releaseDecision(
    consumed: Boolean,
    released: Boolean,
    declaresPressEnd: Boolean,
    declaresPressPair: Boolean,
    tapSentFromOnTap: Boolean = false,
): ReleaseDecision = when {
    tapSentFromOnTap -> ReleaseDecision.Cancel
    consumed && !declaresPressPair -> ReleaseDecision.Cancel
    released || declaresPressEnd -> ReleaseDecision.Fire
    else -> ReleaseDecision.Cancel
}

/** Minimum horizontal travel for a flick to count as a swipe; tiles are
 *  ~150 px on the deck, so 64 px is a deliberate move, not a wiggle. */
const val SWIPE_MIN_PX = 64f
