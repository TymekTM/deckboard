package app.pulpit.mobile

import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.viewmodel.compose.viewModel
import app.pulpit.mobile.state.PulpitViewModel
import app.pulpit.mobile.state.keepsScreenOn
import app.pulpit.mobile.state.showsBoard
import app.pulpit.mobile.ui.BoardScreen
import app.pulpit.mobile.ui.ConnectScreen
import app.pulpit.mobile.ui.DeckboardTheme
import app.pulpit.mobile.ui.ShutdownScreen

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            DeckboardTheme {
                val vm: PulpitViewModel = viewModel()
                val conn by vm.connState.collectAsState()
                val boards by vm.boards.collectAsState()
                val serverDown by vm.serverDown.collectAsState()
                val linkStandby by vm.linkStandby.collectAsState()

                DisposableEffect(LocalLifecycleOwner.current) {
                    val observer = LifecycleEventObserver { _, event ->
                        when (event) {
                            Lifecycle.Event.ON_START -> vm.onAppForeground()
                            Lifecycle.Event.ON_STOP -> vm.onAppBackground()
                            else -> {}
                        }
                    }
                    lifecycle.addObserver(observer)
                    onDispose { lifecycle.removeObserver(observer) }
                }
                // This device is the deck: a live board keeps the display
                // awake. The connect and goodbye screens, and a link dead
                // for LINK_STANDBY_MS, let the system timeout apply - the
                // flag comes back with the next healthy board.
                val board = showsBoard(conn, boards.isNotEmpty(), serverDown)
                val keepOn = keepsScreenOn(board, linkStandby)
                DisposableEffect(keepOn) {
                    if (keepOn) {
                        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    } else {
                        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    }
                    onDispose {}
                }
                when {
                    serverDown -> ShutdownScreen(onTap = vm::reconnectFromShutdown)
                    // With a snapshot on screen the deck stays up while the
                    // link is down (BoardScreen shows the retrying banner);
                    // the connect screen only owns the no-data and refused
                    // states.
                    board -> BoardScreen(vm)
                    else -> ConnectScreen(vm)
                }
            }
        }
    }
}
