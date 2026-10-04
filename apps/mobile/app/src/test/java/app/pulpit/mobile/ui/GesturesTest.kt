//! M5 custom-gesture classification: a swipe is horizontal AND long
//! enough; anything else is None, never a stray trigger.

package app.pulpit.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class GesturesTest {
    @Test
    fun horizontalFlicksClassifyLeftAndRight() {
        assertEquals(Swipe.Left, classifySwipe(-120f, 8f, SWIPE_MIN_PX))
        assertEquals(Swipe.Right, classifySwipe(140f, -12f, SWIPE_MIN_PX))
    }

    @Test
    fun shortDragsAreNotSwipes() {
        assertEquals(Swipe.None, classifySwipe(-40f, 0f, SWIPE_MIN_PX))
        assertEquals(Swipe.None, classifySwipe(63f, 0f, SWIPE_MIN_PX))
    }

    @Test
    fun verticalDragsAreNotSwipes() {
        // dominant axis decides: a long vertical move never becomes a
        // horizontal swipe, even when it travels far
        assertEquals(Swipe.None, classifySwipe(10f, -300f, SWIPE_MIN_PX))
        assertEquals(Swipe.None, classifySwipe(0f, 300f, SWIPE_MIN_PX))
        // borderline diagonal leans vertical -> None
        assertEquals(Swipe.None, classifySwipe(100f, 100f, SWIPE_MIN_PX))
    }

    @Test
    fun trackerSuppressesExactlyOneExtraTap() {
        val tracker = GestureTracker()
        assertFalse(tracker.consumed)
        tracker.longPressed = true
        assertTrue(tracker.consumed)
        tracker.reset()
        assertFalse(tracker.consumed)
        tracker.moved = true
        assertTrue(tracker.consumed)
        tracker.reset()
        tracker.doubleTapped = true
        assertTrue(tracker.consumed)
    }

    // ---- release decision (MOB-02) ----------------------------------

    @Test
    fun consumedGestureStillEndsAPressModeTile() {
        // long-press fired on a key/hold tile: press-end must still be
        // sent - only it stops the server's hold-repeat loop
        assertEquals(
            ReleaseDecision.Fire,
            releaseDecision(consumed = true, released = true, declaresPressEnd = true, declaresPressPair = true),
        )
        // the finger may also have been stolen after the long press -
        // the press-mode tile still owes its press-end
        assertEquals(
            ReleaseDecision.Fire,
            releaseDecision(consumed = true, released = false, declaresPressEnd = true, declaresPressPair = true),
        )
    }

    @Test
    fun consumedGestureSuppressesThePlainTap() {
        assertEquals(
            ReleaseDecision.Cancel,
            releaseDecision(consumed = true, released = true, declaresPressEnd = false, declaresPressPair = false),
        )
    }

    @Test
    fun plainReleaseRulesStayAsTheyWere() {
        // clean release fires; a stolen touch cancels a plain tile...
        assertEquals(
            ReleaseDecision.Fire,
            releaseDecision(consumed = false, released = true, declaresPressEnd = false, declaresPressPair = false),
        )
        assertEquals(
            ReleaseDecision.Cancel,
            releaseDecision(consumed = false, released = false, declaresPressEnd = false, declaresPressPair = false),
        )
        // ...but a press tile still ends a stolen touch (the D1 fix)
        assertEquals(
            ReleaseDecision.Fire,
            releaseDecision(consumed = false, released = false, declaresPressEnd = true, declaresPressPair = true),
        )
    }

    @Test
    fun doubleTapTilesNeverFireFromTheReleasePath() {
        // MOB-03: a double-tap tile without the press pair takes its
        // tap from onTap (deferred through the double-tap window), so
        // the release path cancels even on a clean release - the old
        // code fired tap #1 before onDoubleTap could mark the tracker
        assertEquals(
            ReleaseDecision.Cancel,
            releaseDecision(
                consumed = false,
                released = true,
                declaresPressEnd = false,
                declaresPressPair = false,
                tapSentFromOnTap = true,
            ),
        )
    }
}
