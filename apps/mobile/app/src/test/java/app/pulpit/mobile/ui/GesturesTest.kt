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
}
