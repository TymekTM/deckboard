//! LinkPolicy tests: the pure background/keep-alive decisions and the
//! notification line (ROADMAP M4 vs plan 008).

package app.pulpit.mobile

import app.pulpit.mobile.net.ConnState
import app.pulpit.mobile.state.closesLinkOnBackground
import app.pulpit.mobile.state.keepsScreenOn
import app.pulpit.mobile.state.linkStatusLine
import app.pulpit.mobile.state.showsBoard
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LinkPolicyTest {
    @Test
    fun backgroundClosesTheLinkWithoutKeepAlive() {
        assertTrue(closesLinkOnBackground(keepAlive = false, refused = false, pairingInFlight = false))
    }

    @Test
    fun keepAliveHoldsTheLinkInBackground() {
        assertFalse(closesLinkOnBackground(keepAlive = true, refused = false, pairingInFlight = false))
    }

    @Test
    fun aRefusalOrPairingInFlightAlwaysCloses() {
        // a refused pairing must keep its message on the connect screen,
        // and a one-time code in flight cannot be retried anyway - the
        // keep-alive service must not override either
        assertTrue(closesLinkOnBackground(keepAlive = true, refused = true, pairingInFlight = false))
        assertTrue(closesLinkOnBackground(keepAlive = true, refused = false, pairingInFlight = true))
    }

    @Test
    fun statusLineNamesEveryState() {
        assertEquals("połączony z 192.0.2.1", linkStatusLine(ConnState.Connected("192.0.2.1", 8500), "192.0.2.1"))
        assertEquals("łączy się z 192.0.2.1...", linkStatusLine(ConnState.Connecting("192.0.2.1", 8500), "192.0.2.1"))
        assertEquals("serwer zamknięty - czeka", linkStatusLine(ConnState.ServerDown, "h"))
        assertEquals("serwer nieosiągalny - próbuje dalej", linkStatusLine(ConnState.Failed("timeout"), "h"))
        assertEquals("parowanie odrzucone", linkStatusLine(ConnState.Failed("bad code", retryable = false), "h"))
        assertEquals("rozłączony", linkStatusLine(ConnState.Disconnected, "h"))
    }

    @Test
    fun screenPolicyStillSane() {
        // a live board keeps the screen awake; a board in standby does not
        assertTrue(showsBoard(ConnState.Connected("h", 8500), hasBoards = false, serverDown = false))
        assertFalse(keepsScreenOn(showsBoard = true, linkStandby = true))
        assertTrue(keepsScreenOn(showsBoard = true, linkStandby = false))
    }
}
