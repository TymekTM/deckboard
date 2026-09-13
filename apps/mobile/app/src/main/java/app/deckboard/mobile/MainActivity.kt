package app.deckboard.mobile

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.lifecycle.viewmodel.compose.viewModel
import app.deckboard.mobile.state.DeckboardViewModel
import app.deckboard.mobile.ui.BoardScreen
import app.deckboard.mobile.ui.ConnectScreen
import app.deckboard.mobile.ui.DeckboardTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            DeckboardTheme {
                val vm: DeckboardViewModel = viewModel()
                val conn by vm.connState.collectAsState()
                when (conn) {
                    is app.deckboard.mobile.net.ConnState.Connected,
                    is app.deckboard.mobile.net.ConnState.Connecting,
                    -> BoardScreen(vm)
                    else -> ConnectScreen(vm, onConnected = {})
                }
            }
        }
    }
}
