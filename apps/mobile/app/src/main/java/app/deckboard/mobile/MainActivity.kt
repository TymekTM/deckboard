package app.deckboard.mobile

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
import app.deckboard.mobile.state.DeckboardViewModel
import app.deckboard.mobile.ui.BoardScreen
import app.deckboard.mobile.ui.ConnectScreen
import app.deckboard.mobile.ui.DeckboardTheme
import app.deckboard.mobile.ui.ShutdownScreen

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // This device is the deck - keep the board visible, not dozing.
        // The shutdown overlay drops the flag again: nothing to watch, so
        // the screen may sleep (re-added when the deck comes back).
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        setContent {
            DeckboardTheme {
                val vm: DeckboardViewModel = viewModel()
                val conn by vm.connState.collectAsState()
                val boards by vm.boards.collectAsState()
                val serverDown by vm.serverDown.collectAsState()

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
                DisposableEffect(serverDown) {
                    if (serverDown) {
                        window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    } else {
                        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                    }
                    onDispose {}
                }

                if (serverDown) {
                    ShutdownScreen(onTap = vm::reconnectFromShutdown)
                } else if (conn is app.deckboard.mobile.net.ConnState.Connected || boards.isNotEmpty()) {
                    // With a snapshot on screen the deck stays up while the
                    // link is down (BoardScreen shows the retrying banner);
                    // the connect screen only owns the no-data states.
                    BoardScreen(vm)
                } else {
                    ConnectScreen(vm, onConnected = {})
                }
            }
        }
    }
}
