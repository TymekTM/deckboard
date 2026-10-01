package app.pulpit.mobile.state

import app.pulpit.mobile.net.ConnState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Screen-choice and keep-screen-on policy the Activity follows. */
class LinkPolicyTest {

    @Test
    fun liveBoardShows() {
        assertTrue(showsBoard(ConnState.Connected("h", 1), hasBoards = false, serverDown = false))
    }

    @Test
    fun snapshotRidesOutADrop() {
        assertTrue(showsBoard(ConnState.Disconnected, hasBoards = true, serverDown = false))
        assertTrue(showsBoard(ConnState.Connecting("h", 1), hasBoards = true, serverDown = false))
        assertTrue(showsBoard(ConnState.Failed("timeout"), hasBoards = true, serverDown = false))
    }

    @Test
    fun noSnapshotMeansConnectScreen() {
        assertFalse(showsBoard(ConnState.Disconnected, hasBoards = false, serverDown = false))
    }

    @Test
    fun goodbyeWins() {
        assertFalse(showsBoard(ConnState.Connected("h", 1), hasBoards = true, serverDown = true))
    }

    @Test
    fun refusalShowsConnectScreen() {
        assertFalse(showsBoard(ConnState.Failed("x", retryable = false), hasBoards = true, serverDown = false))
    }

    @Test
    fun screenHeldOnlyForBoardOutsideStandby() {
        assertTrue(keepsScreenOn(showsBoard = true, linkStandby = false))
        assertFalse(keepsScreenOn(showsBoard = true, linkStandby = true))
        assertFalse(keepsScreenOn(showsBoard = false, linkStandby = false))
        assertFalse(keepsScreenOn(showsBoard = false, linkStandby = true))
    }

    @Test
    fun standbyAfterThreeMinutes() {
        assertEquals(180_000L, LINK_STANDBY_MS)
    }
}
