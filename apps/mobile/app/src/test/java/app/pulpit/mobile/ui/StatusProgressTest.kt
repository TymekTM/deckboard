package app.pulpit.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Test

/** The client-side playback extrapolation (design §4): the payload has
 *  no server timestamp, so the receiver stamps arrival locally and the
 *  displayed position advances by elapsed wall time while playing -
 *  clamped to the track - and freezes while paused. */
class StatusProgressTest {

    private val playing = StatusProgress(
        positionMs = 60_000,
        durationMs = 200_000,
        playing = true,
    )
    private val paused = playing.copy(playing = false)

    @Test
    fun positionAdvancesByElapsedTimeWhilePlaying() {
        // arrived at t=1000, asked at t=61000: one minute of music on
        assertEquals(120_000, statusProgressAt(playing, receivedAtMs = 1_000, nowMs = 61_000))
        // small steps advance in kind
        assertEquals(61_234, statusProgressAt(playing, receivedAtMs = 1_000, nowMs = 2_234))
    }

    @Test
    fun positionNeverPassesTheTrackEnd() {
        assertEquals(
            200_000,
            statusProgressAt(playing, receivedAtMs = 1_000, nowMs = 5_000_000),
        )
    }

    @Test
    fun pausedFreezesAtTheReportedPosition() {
        assertEquals(60_000, statusProgressAt(paused, receivedAtMs = 1_000, nowMs = 61_000))
        assertEquals(60_000, statusProgressAt(paused, receivedAtMs = 1_000, nowMs = 9_999_999))
    }

    @Test
    fun wonkyArrivalValuesDoNotJumpTheBar() {
        // a payload stamped after "now" (clock read in a different order)
        // must not rewind: the elapsed part floors at zero
        assertEquals(60_000, statusProgressAt(playing, receivedAtMs = 5_000, nowMs = 4_000))
        // a producer reporting a position past the end clamps even paused
        val overrun = StatusProgress(250_000, durationMs = 200_000, playing = false)
        assertEquals(200_000, statusProgressAt(overrun, receivedAtMs = 0, nowMs = 10))
        // ...and a negative one clamps up to the track start
        val underrun = StatusProgress(-3_000, durationMs = 200_000, playing = false)
        assertEquals(0, statusProgressAt(underrun, receivedAtMs = 0, nowMs = 10))
    }

    @Test
    fun mmssFormatsPlaybackClocks() {
        assertEquals("0:00", mmss(0))
        assertEquals("0:05", mmss(5_400))
        assertEquals("1:01", mmss(61_000))
        assertEquals("3:21", mmss(201_000))
        // negative input cannot happen on the wire; formats as 0:00
        assertEquals("0:00", mmss(-1))
    }
}
