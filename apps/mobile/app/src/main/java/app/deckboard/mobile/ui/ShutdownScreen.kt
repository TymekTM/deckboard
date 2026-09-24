//! The standby screen after the server announced its shutdown: full black
//! with the hour-appropriate goodbye. A tap anywhere leaves standby and
//! reconnects; the screen can also just doze (the Activity drops
//! keep-screen-on while this is up).

package app.deckboard.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput

/** Greeting for the standby screen: late evening and night say goodnight,
 *  everything else goodbye. [hour] is the tablet's local 0-23. */
fun shutdownGreeting(hour: Int): String =
    if (hour >= 21 || hour < 6) "Goodnight" else "Goodbye"

@Composable
fun ShutdownScreen(onTap: () -> Unit) {
    // Captured once per standby visit: the greeting reflects when the
    // server went down, not whenever a recomposition happens to run.
    val title = remember {
        val hour = java.util.Calendar.getInstance().get(java.util.Calendar.HOUR_OF_DAY)
        shutdownGreeting(hour)
    }
    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black)
            .pointerInput(Unit) { detectTapGestures { onTap() } },
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(
            text = title,
            style = MaterialTheme.typography.headlineLarge,
            color = Color.White,
        )
        Text(
            text = "The server shut down. Tap anywhere to reconnect.",
            style = MaterialTheme.typography.bodySmall,
            color = Color.White.copy(alpha = 0.6f),
        )
    }
}
