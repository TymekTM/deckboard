package app.pulpit.mobile

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.lifecycle.viewmodel.compose.viewModel
import app.pulpit.mobile.state.PulpitViewModel
import app.pulpit.mobile.ui.BoardScreen
import app.pulpit.mobile.ui.ConnectScreen
import app.pulpit.mobile.ui.PulpitTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            PulpitTheme {
                val vm: PulpitViewModel = viewModel()
                val conn by vm.connState.collectAsState()
                when (conn) {
                    is app.pulpit.mobile.net.ConnState.Connected,
                    is app.pulpit.mobile.net.ConnState.Connecting,
                    -> BoardScreen(vm)
                    else -> ConnectScreen(vm, onConnected = {})
                }
            }
        }
    }
}
