//! Asset response read cap (round 4, MOB-12): a multi-MB photo must
//! not buffer whole into the heap of a 1 GB tablet before the sampled
//! decode - the decode was capped all along, the read was not.

package app.pulpit.mobile.state

import java.io.ByteArrayOutputStream
import java.io.InputStream

/** Reads up to [cap] bytes from [input]; null when the stream carries
 *  more than the cap (the caller treats an oversized asset as a failed
 *  fetch - same path as an HTTP error, retried with backoff). */
fun readAtMost(input: InputStream, cap: Int): ByteArray? {
    val out = ByteArrayOutputStream(minOf(cap, 64 * 1024))
    val buf = ByteArray(64 * 1024)
    var total = 0
    while (true) {
        val n = input.read(buf)
        if (n < 0) break
        total += n
        if (total > cap) return null
        out.write(buf, 0, n)
    }
    return out.toByteArray()
}
