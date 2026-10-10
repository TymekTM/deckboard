package app.pulpit.mobile

import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.platform.LocalView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.viewmodel.compose.viewModel
import app.pulpit.mobile.state.PulpitViewModel
import app.pulpit.mobile.state.keepsScreenOn
import app.pulpit.mobile.state.showsBoard
import app.pulpit.mobile.ui.AndroidHaptics
import app.pulpit.mobile.ui.BoardScreen
import app.pulpit.mobile.ui.ConnectScreen
import app.pulpit.mobile.ui.DeckboardTheme
import app.pulpit.mobile.ui.LocalHaptics
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

                val hapticConfig by vm.hapticConfig.collectAsState()
                val view = LocalView.current
                val context = LocalContext.current
                val haptics = remember(view, context, hapticConfig) {
                    AndroidHaptics(viewProvider = { view }, context = context, configProvider = { hapticConfig })
                }

                CompositionLocalProvider(LocalHaptics provides haptics) {
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
                    val menuOpen by vm.menuOpen.collectAsState()
                    val board = showsBoard(conn, boards.isNotEmpty(), serverDown, menuOpen)
                    val keepOn = keepsScreenOn(board, linkStandby)
                    DisposableEffect(keepOn) {
                        if (keepOn) {
                            window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                        } else {
                            window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                        }
                        onDispose {}
                    }
                    // Back on the deck must not close the app: a tablet that
                    // cannot reconnect would be left without a way to the
                    // address fields. Ask, then fall back to the connect
                    // screen; Back there still leaves the app.
                    var confirmExit by remember { mutableStateOf(false) }
                    BackHandler(enabled = board || serverDown) { confirmExit = true }
                    if (confirmExit && (board || serverDown)) {
                        AlertDialog(
                            onDismissRequest = { confirmExit = false },
                            title = { Text("Wyjść do menu głównego?") },
                            text = { Text("Połączenie z komputerem zostanie zamknięte.") },
                            confirmButton = {
                                TextButton(onClick = {
                                    confirmExit = false
                                    vm.openMenu()
                                }) { Text("Tak") }
                            },
                            dismissButton = {
                                TextButton(onClick = { confirmExit = false }) { Text("Nie") }
                            },
                        )
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
}