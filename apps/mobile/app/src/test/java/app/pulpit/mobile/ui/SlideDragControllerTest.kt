//! MOB-13: the drag protocol shared by sliders and knobs, tested once
//! instead of twice (the protocol used to be copy-pasted between the
//! two templates).

package app.pulpit.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SlideDragControllerTest {
    private fun controller(throttleMs: Long = 60_000) =
        SlideDragController(SlideThrottle(throttleMs))

    @Test
    fun followsLiveUntilTouched() {
        val slide = controller()
        assertEquals(0.5f, slide.current(null))
        assertEquals(0.25f, slide.current(0.25))
        // out-of-range live values clamp, like the old templates
        assertEquals(1f, slide.current(7.0))
        assertEquals(0f, slide.current(-7.0))
    }

    @Test
    fun startAlwaysSendsAndHoldsTheValue() {
        val sent = mutableListOf<Float>()
        val slide = controller()
        slide.start(0.4f, sent::add)
        assertEquals(listOf(0.4f), sent)
        assertEquals(0.4f, slide.current(0.9))
    }

    @Test
    fun movesAreThrottledButEndConverges() {
        val sent = mutableListOf<Float>()
        val slide = controller()
        slide.start(0.1f, sent::add)
        slide.move(0.2f, sent::add) // inside the window - dropped
        slide.move(0.8f, sent::add) // dropped too
        assertEquals(listOf(0.1f), sent)
        // release always delivers the last sampled value
        slide.end(live = 0.9, send = sent::add)
        assertEquals(listOf(0.1f, 0.8f), sent)
        // and a completed drag keeps its position (no echo flicker)
        assertEquals(0.8f, slide.current(0.9))
    }

    @Test
    fun cancelCommitsThenHandsControlBackToLive() {
        val sent = mutableListOf<Float>()
        val slide = controller()
        slide.start(0.3f, sent::add)
        slide.move(0.6f, sent::add)
        slide.cancel(live = 0.2, send = sent::add)
        assertEquals(listOf(0.3f, 0.6f), sent)
        assertEquals(0.2f, slide.current(0.2))
    }

    @Test
    fun endWithoutAnyDragSendsTheLiveValue() {
        val sent = mutableListOf<Float>()
        val slide = controller()
        slide.end(live = 0.7, send = sent::add)
        assertEquals(listOf(0.7f), sent)
        assertNull(slide.dragValue)
    }
}
