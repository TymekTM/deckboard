//! MOB-11: the slide throttle drops sends inside its window and always
//! lets the forced final value through - release/cancel convergence.

package app.pulpit.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SlideThrottleTest {
    @Test
    fun sendsInsideTheWindowAreDropped() {
        val sent = mutableListOf<Float>()
        val throttle = SlideThrottle(throttleMs = 60_000)
        throttle.push(0.1f, send = sent::add)
        throttle.push(0.2f, send = sent::add)
        throttle.push(0.3f, send = sent::add)
        assertEquals(listOf(0.1f), sent)
    }

    @Test
    fun forceAlwaysSends() {
        val sent = mutableListOf<Float>()
        val throttle = SlideThrottle(throttleMs = 60_000)
        throttle.push(0.1f, send = sent::add)
        throttle.push(0.9f, force = true, send = sent::add)
        assertEquals(listOf(0.1f, 0.9f), sent)
    }

    @Test
    fun anElapsedWindowSendsAgain() {
        val sent = mutableListOf<Float>()
        val throttle = SlideThrottle(throttleMs = 0)
        throttle.push(0.1f, send = sent::add)
        throttle.push(0.2f, send = sent::add)
        assertEquals(listOf(0.1f, 0.2f), sent)
    }

    @Test
    fun defaultCadenceStaysNearTheDesktop() {
        // the desktop sends at most every 150 ms (TileCell.vue); the
        // deck must not drift back toward one frame per drag event
        assertTrue(SLIDE_THROTTLE_MS in 100..150)
        // and the default constructor throttles with it: an immediate
        // follow-up push is dropped
        val sent = mutableListOf<Float>()
        val throttle = SlideThrottle()
        throttle.push(0.1f, send = sent::add)
        throttle.push(0.2f, send = sent::add)
        assertEquals(listOf(0.1f), sent)
    }
}
