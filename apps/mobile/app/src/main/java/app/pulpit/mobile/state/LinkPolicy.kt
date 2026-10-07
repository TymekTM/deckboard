//! Deck screen policy (pure, unit-tested in LinkPolicyTest): which screen
//! the Activity shows, and when it may let the display sleep.

package app.pulpit.mobile.state

import android.content.Context
import app.pulpit.mobile.net.ConnState
import kotlinx.coroutines.flow.MutableStateFlow

/** How long the link may stay down, with the board on screen, before the
 *  deck stops holding the display awake. Long enough to ride out a Wi-Fi
 *  blip or a desktop restart; short enough that a sleeping PC does not
 *  keep the tablet lit all night. */
const val LINK_STANDBY_MS = 3 * 60_000L

/** Shared flags between the ViewModel (which owns the socket) and
 *  [app.pulpit.mobile.LinkService] (which owns the foreground state).
 *  The service flips [keepLinkInBackground]; the ViewModel mirrors the
 *  link line into [status] for the notification. */
object LinkBus {
    val status = MutableStateFlow("")

    /** True while the keep-alive service is up: background keeps the
     *  link open (see [closesLinkOnBackground]). */
    val keepLinkInBackground = MutableStateFlow(false)
}

/** Whether the board (not the connect or goodbye screen) is on screen. A
 *  snapshot keeps the board up through a transient drop; a refusal
 *  (revoked token, bad code) needs the connect screen and its "Forget
 *  pairing". [menuOpen] is the user's own back-button exit to the connect
 *  screen: it wins over any snapshot, so a deck that cannot reconnect can
 *  still reach the address fields. */
fun showsBoard(
    conn: ConnState,
    hasBoards: Boolean,
    serverDown: Boolean,
    menuOpen: Boolean = false,
): Boolean {
    if (serverDown || menuOpen) return false
    if (conn is ConnState.Failed && !conn.retryable) return false
    return conn is ConnState.Connected || hasBoards
}

/** FLAG_KEEP_SCREEN_ON is held only for a board that is live or still
 *  inside its reconnect grace; everything else follows the system
 *  screen timeout. */
fun keepsScreenOn(showsBoard: Boolean, linkStandby: Boolean): Boolean =
    showsBoard && !linkStandby

/** One line for the keep-alive service notification (and Plog): what
 *  the link is doing, in deck terms. */
fun linkStatusLine(conn: ConnState, host: String): String = when (conn) {
    is ConnState.Connected -> "połączony z $host"
    is ConnState.Connecting -> "łączy się z $host..."
    is ConnState.ServerDown -> "serwer zamknięty - czeka"
    is ConnState.Failed ->
        if (conn.retryable) "serwer nieosiągalny - próbuje dalej" else "parowanie odrzucone"
    is ConnState.Disconnected -> "rozłączony"
}

/** Pure background decision (ROADMAP M4 vs plan 008): the socket closes
 *  on background - 008's battery win - unless the foreground keep-alive
 *  service is running. A final refusal or a pairing in flight always
 *  closes: the connect screen must keep its message, and a one-time
 *  code cannot be retried anyway. */
fun closesLinkOnBackground(keepAlive: Boolean, refused: Boolean, pairingInFlight: Boolean): Boolean =
    !(keepAlive && !refused && !pairingInFlight)
