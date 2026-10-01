package app.pulpit.mobile.net

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Terminal-vs-retryable classification of connection states. */
class ConnStateTest {

    @Test
    fun failedIsRetryableByDefault() {
        assertTrue(ConnState.Failed("x").retryable)
        assertFalse(ConnState.Failed("x").isTerminal())
    }

    @Test
    fun refusalAndGoodbyeAreTerminal() {
        assertTrue(ConnState.Failed("x", retryable = false).isTerminal())
        assertTrue(ConnState.ServerDown.isTerminal())
        assertFalse(ConnState.Disconnected.isTerminal())
        assertFalse(ConnState.Connecting("h", 1).isTerminal())
        assertFalse(ConnState.Connected("h", 1).isTerminal())
    }

    @Test
    fun http401IsTerminalRefusal() {
        val st = failureState(401, "boom")
        assertFalse(st.retryable)
        assertEquals(fatalReason("unauthorized"), st.reason)
    }

    @Test
    fun otherFailuresStayRetryable() {
        assertEquals("timeout", failureState(null, "timeout").reason)
        assertTrue(failureState(null, "timeout").retryable)
        val st = failureState(500, null)
        assertTrue(st.retryable)
        assertEquals("connection failed", st.reason)
    }

    @Test
    fun fatalReasonKeepsTheOldWording() {
        assertEquals("invalid pairing code - generate a new one on the desktop", fatalReason("pair-invalid"))
        assertEquals("pairing code expired - generate a new one on the desktop", fatalReason("pair-expired"))
        assertEquals("device revoked on the desktop - pair again", fatalReason("unauthorized"))
        assertTrue(fatalReason("too-large").contains("too-large"))
    }

    @Test
    fun everyFatalCodeHasAReason() {
        for (code in V2Client.FATAL_CODES) {
            assertTrue(fatalReason(code).isNotBlank())
        }
    }
}
