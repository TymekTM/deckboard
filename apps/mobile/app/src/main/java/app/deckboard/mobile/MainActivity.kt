package app.deckboard.mobile

import android.os.Bundle
import android.view.WindowManager
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
        // This device is the deck - keep the board visible, not dozing.
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        setContent {
            DeckboardTheme {
                val vm: DeckboardViewModel = viewModel()
                val conn by vm.connState.collectAsState()
                // Only a completed hello/welcome means the board screen has
                // data; Connecting stays on the connect screen (spinner).
                if (conn is app.deckboard.mobile.net.ConnState.Connected) {
                    BoardScreen(vm)
                } else {
                    ConnectScreen(vm, onConnected = {})
                }
            }
        }
    }
}
