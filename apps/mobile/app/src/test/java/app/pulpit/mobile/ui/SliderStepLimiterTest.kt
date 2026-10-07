//! Unit tests for pure slider step rate-limiter and its SlideDragController integration.

package app.pulpit.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SliderStepLimiterTest {

    @Test
    fun movesWithinSameStepDoNotTriggerTick() {
        var currentTime = 1000L
        val limiter = SliderStepLimiter(
            step = 0.05f,
            rateLimitMs = 50L,
            clock = { currentTime },
        )
        limiter.reset(0.50f)

        // 0.50f to 0.52f is within step 10 (0.50..0.549)
        assertFalse(limiter.onMove(0.52f))
        assertFalse(limiter.onMove(0.54f))
    }

    @Test
    fun stepCrossingTriggersTick() {
        var currentTime = 1000L
        val limiter = SliderStepLimiter(
            step = 0.05f,
            rateLimitMs = 50L,
            clock = { currentTime },
        )
        limiter.reset(0.50f)

        // Cross boundary into step 11 (0.55)
        currentTime = 1100L
        assertTrue(limiter.onMove(0.56f))
    }

    @Test
    fun rateLimiterDropsFastTicks() {
        var currentTime = 1000L
        val limiter = SliderStepLimiter(
            step = 0.05f,
            rateLimitMs = 50L,
            clock = { currentTime },
        )
        limiter.reset(0.50f)

        // First step crossing at t=1000 fires
        currentTime = 1000L
        assertTrue(limiter.onMove(0.56f))

        // Fast flick 10ms later into step 12 is rate-limited
        currentTime = 1010L
        assertFalse(limiter.onMove(0.62f))

        // Another fast flick 20ms later into step 13 is rate-limited
        currentTime = 1030L
        assertFalse(limiter.onMove(0.67f))

        // Once 50ms rateLimit has elapsed (t=1060), moving to step 14 fires
        currentTime = 1060L
        assertTrue(limiter.onMove(0.72f))
    }

    @Test
    fun reverseDragCrossesBoundary() {
        var currentTime = 1000L
        val limiter = SliderStepLimiter(
            step = 0.05f,
            rateLimitMs = 50L,
            clock = { currentTime },
        )
        limiter.reset(0.60f)

        currentTime = 1100L
        // Dragging down from 0.60 (step 12) to 0.53 (step 10)
        assertTrue(limiter.onMove(0.53f))
    }

    @Test
    fun clampingAtExtremes() {
        var currentTime = 1000L
        val limiter = SliderStepLimiter(
            step = 0.05f,
            rateLimitMs = 50L,
            clock = { currentTime },
        )
        limiter.reset(0.95f)

        currentTime = 1100L
        // Moving to 1.0f (step 20) triggers
        assertTrue(limiter.onMove(1.0f))

        // Out-of-bounds clamp to 1.0f stays at step 20
        currentTime = 1200L
        assertFalse(limiter.onMove(1.5f))

        // Moving to 0.0f
        currentTime = 1300L
        assertTrue(limiter.onMove(0.0f))

        // Out-of-bounds clamp to 0.0f stays at step 0
        currentTime = 1400L
        assertFalse(limiter.onMove(-0.5f))
    }

    @Test
    fun initialResetDoesNotSpuriousTick() {
        val limiter = SliderStepLimiter(step = 0.05f, rateLimitMs = 50L)
        limiter.reset(0.30f)
        // Re-evaluating starting value
        assertFalse(limiter.onMove(0.30f))
    }

    @Test
    fun slideDragControllerInvokesOnStepTick() {
        var currentTime = 1000L
        val limiter = SliderStepLimiter(
            step = 0.05f,
            rateLimitMs = 50L,
            clock = { currentTime },
        )
        val controller = SlideDragController(
            throttle = SlideThrottle(throttleMs = 0),
            stepLimiter = limiter,
        )

        var ticks = 0
        val sends = mutableListOf<Float>()

        controller.start(0.20f) { sends.add(it) }

        // Move inside same bucket
        controller.move(0.22f, onStepTick = { ticks++ }) { sends.add(it) }
        assertEquals(0, ticks)

        // Move across bucket
        currentTime = 1100L
        controller.move(0.30f, onStepTick = { ticks++ }) { sends.add(it) }
        assertEquals(1, ticks)
    }
}