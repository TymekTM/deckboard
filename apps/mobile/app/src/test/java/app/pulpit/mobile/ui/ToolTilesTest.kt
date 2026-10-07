package app.pulpit.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Test

/** Pure extrapolation/format helpers of the tool tiles (ToolTiles.kt). */
class ToolTilesTest {

    @Test
    fun timerRemainingExtrapolatesWhileRunning() {
        // 60 s countdown, 15 s accumulated, running for 10 s more
        assertEquals(
            35_000L,
            timerExtrapolateRemaining(
                durationMs = 60_000L,
                elapsedMs = 15_000L,
                startedAtMs = 1_000L,
                running = true,
                finished = false,
                nowMs = 11_000L,
            ),
        )
    }

    @Test
    fun timerRemainingFreezesWhilePausedAndAtFinish() {
        assertEquals(
            45_000L,
            timerExtrapolateRemaining(60_000L, 15_000L, 1_000L, running = false, finished = false, nowMs = 999_999L),
        )
        assertEquals(
            0L,
            timerExtrapolateRemaining(60_000L, 0L, 0L, running = false, finished = true, nowMs = 5L),
        )
        // clock behind the start stamp must not produce negative time
        assertEquals(
            60_000L,
            timerExtrapolateRemaining(60_000L, 0L, 5_000L, running = true, finished = false, nowMs = 1_000L),
        )
    }

    @Test
    fun stopwatchElapsedExtrapolatesOnlyWhileRunning() {
        assertEquals(
            25_000L,
            stopwatchExtrapolateElapsed(elapsedMs = 5_000L, startedAtMs = 1_000L, running = true, nowMs = 21_000L),
        )
        assertEquals(
            5_000L,
            stopwatchExtrapolateElapsed(elapsedMs = 5_000L, startedAtMs = 1_000L, running = false, nowMs = 999_999L),
        )
    }

    @Test
    fun mmssFormatsWithAndWithoutHours() {
        assertEquals("00:05", formatMmSs(5_000L))
        // ceil so a running timer never shows one second too many
        assertEquals("01:00", formatMmSs(59_001L))
        assertEquals("01:05", formatMmSs(65_000L))
        assertEquals("01:00:00", formatMmSs(3_600_000L))
    }

    @Test
    fun durationStringsParseLikeTheServer() {
        assertEquals(300_000L, parseDurationString("05:00"))
        assertEquals(90_000L, parseDurationString("1:30"))
        assertEquals(45_000L, parseDurationString("45"))
        assertEquals(3_600_000L, parseDurationString("1:00:00"))
        // unusable input falls back to the 5 min default, like the server
        assertEquals(300_000L, parseDurationString("abc"))
        assertEquals(300_000L, parseDurationString(null))
    }
}
