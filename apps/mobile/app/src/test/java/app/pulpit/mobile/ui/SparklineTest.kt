//! MOB-08: the sparkline normalization must mirror the desktop's
//! TileCell.vue sparkPoints - y over the window's own min..max with a
//! 5% pad - so the same series draws the same amplitude on both
//! surfaces.

package app.pulpit.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class SparklineTest {
    @Test
    fun minAndMaxSitOnThePaddedEdges() {
        val min = 2.0
        val span = 2.0 // series 2..4
        // min -> 95% down (the pad), max -> 5% from the top
        assertEquals(0.95, sparkY(2.0, min, span), 1e-9)
        assertEquals(0.05, sparkY(4.0, min, span), 1e-9)
        // mid value at mid-height
        assertEquals(0.5, sparkY(3.0, min, span), 1e-9)
    }

    @Test
    fun anIdlingSeriesSpansTheFullTile() {
        // the old average-centered rule with floors (half = max(dev,
        // 0.2*|avg|, 5)) squeezed a 2..4% series into a fraction of the
        // tile; min..max normalization uses the whole height like the
        // desktop (y grows downward, so the window min sits lower)
        val min = 2.0
        val span = 2.0
        val atMin = sparkY(2.0, min, span)
        val atMax = sparkY(4.0, min, span)
        assertEquals(0.90, atMin - atMax, 1e-9)
    }

    @Test
    fun monotonicBetweenTheEdges() {
        val min = 10.0
        val span = 30.0
        var last = sparkY(min, min, span)
        for (i in 1..10) {
            val y = sparkY(min + span * i / 10.0, min, span)
            assertEquals(true, y < last)
            last = y
        }
    }
}
