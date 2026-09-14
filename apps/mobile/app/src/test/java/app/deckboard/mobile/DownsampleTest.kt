package app.deckboard.mobile.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Chart bucket-averaging: the server window (120 points) is denser than
 *  a tile can show, so it is downsampled before drawing. */
class DownsampleTest {

    @Test
    fun shortWindowsPassThrough() {
        val history = listOf(1.0, 2.0, 3.0)
        assertEquals(history, downsample(history, 40))
    }

    @Test
    fun bucketCountIsCappedAndOrderKept() {
        val history = (1..120).map { it.toDouble() }
        val points = downsample(history, 40)
        assertEquals(40, points.size)
        assertTrue(points.zipWithNext().all { (a, b) -> a <= b })
        // buckets average their members, so ends land mid-bucket
        assertEquals(2.0, points.first(), 1e-9)
        assertEquals(119.0, points.last(), 1e-9)
    }

    @Test
    fun bucketsAverageTheirRange() {
        // 4 points into 2 buckets: each bucket averages its two members
        val history = listOf(2.0, 4.0, 10.0, 20.0)
        val points = downsample(history, 2)
        assertEquals(listOf(3.0, 15.0), points)
    }
}
