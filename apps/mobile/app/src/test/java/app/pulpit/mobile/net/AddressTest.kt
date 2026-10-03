//! MOB-01: the address oracle for the connect path. Whatever passes
//! addressError can build the ws URL OkHttp needs; whatever fails must
//! die as a ConnState.Failed, never as an IllegalArgumentException from
//! Request.Builder().url() inside a click handler or the launch path.

package app.pulpit.mobile.net

import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

class AddressTest {
    @Test
    fun addressesOkHttpAcceptsPass() {
        assertNull(addressError("192.168.1.2", 8500))
        assertNull(addressError("127.0.0.1", 1))
        assertNull(addressError("desktop", 8500))
        assertNull(addressError("my-pc.local", 80))
        assertNull(addressError("192.168.1.2", 65535))
    }

    @Test
    fun junkHostsAreRejected() {
        // a space, a path, or any other character OkHttp will not parse
        // as an authority must fail here instead of throwing later
        assertNotNull(addressError("my pc", 8500))
        assertNotNull(addressError("192.168.1.2/x", 8500))
        assertNotNull(addressError("", 8500))
        assertNotNull(addressError("   ", 8500))
    }

    @Test
    fun portRangeIsEnforced() {
        // "99999" passes the 5-digit filter in the port field
        assertNotNull(addressError("192.168.1.2", 99999))
        assertNotNull(addressError("192.168.1.2", 65536))
        assertNotNull(addressError("192.168.1.2", 0))
        // 0 also stands in for "no parseable number" from the field
        assertNotNull(addressError("192.168.1.2", 0))
        assertNull(addressError("192.168.1.2", 8500))
    }
}
