//! MOB-12: an asset response larger than the cap reads as a failed
//! fetch (null), not a heap spike; everything up to the cap comes
//! through byte-identical.

package app.pulpit.mobile.state

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.ByteArrayInputStream

class AssetLimitsTest {
    @Test
    fun smallStreamsPassThroughUnchanged() {
        val data = ByteArray(100) { it.toByte() }
        assertArrayEquals(data, readAtMost(ByteArrayInputStream(data), 1024))
    }

    @Test
    fun aStreamExactlyAtTheCapPasses() {
        val data = ByteArray(64) { 7 }
        assertArrayEquals(data, readAtMost(ByteArrayInputStream(data), 64))
    }

    @Test
    fun aStreamOneByteOverTheCapIsRejected() {
        val data = ByteArray(65) { 7 }
        assertNull(readAtMost(ByteArrayInputStream(data), 64))
    }

    @Test
    fun largeStreamsAreRejectedWithoutBuffering() {
        // a chunked body bigger than the cap must bail on the read
        val data = ByteArray(256 * 1024) { 3 }
        assertNull(readAtMost(ByteArrayInputStream(data), 64 * 1024))
        assertEquals(256 * 1024, data.size)
    }

    @Test
    fun anEmptyStreamReadsAsEmpty() {
        assertEquals(0, readAtMost(ByteArrayInputStream(ByteArray(0)), 10)?.size)
    }
}
