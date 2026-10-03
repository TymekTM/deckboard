//! Deck screen policy (pure, unit-tested in LinkPolicyTest): which screen
//! the Activity shows, and when it may let the display sleep.

package app.pulpit.mobile.state

import app.pulpit.mobile.net.ConnState

/** How long the link may stay down, with the board on screen, before the
 *  deck stops holding the display awake. Long enough to ride out a Wi-Fi
 *  blip or a desktop restart; short enough that a sleeping PC does not
 *  keep the tablet lit all night. */
const val LINK_STANDBY_MS = 3 * 60_000L

/** Whether the board (not the connect or goodbye screen) is on screen. A
 *  snapshot keeps the board up through a transient drop; a refusal
 *  (revoked token, bad code) needs the connect screen and its "Forget
 *  pairing". */
fun showsBoard(conn: ConnState, hasBoards: Boolean, serverDown: Boolean): Boolean {
    if (serverDown) return false
    if (conn is ConnState.Failed && !conn.retryable) return false
    return conn is ConnState.Connected || hasBoards
}

/** FLAG_KEEP_SCREEN_ON is held only for a board that is live or still
 *  inside its reconnect grace; everything else follows the system
 *  screen timeout. */
fun keepsScreenOn(showsBoard: Boolean, linkStandby: Boolean): Boolean =
    showsBoard && !linkStandby
