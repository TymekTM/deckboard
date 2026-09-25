//! First-run / disconnected screen: server address entry plus pairing.
//! A paired device (token stored) just hits Connect. A new device enters
//! the one-time code the desktop prints next to its QR (POST /v2/pair)
//! together with a name shown in the desktop's trust prompt.

package app.pulpit.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.pulpit.mobile.net.ConnState
import app.pulpit.mobile.state.PulpitViewModel

@Composable
fun ConnectScreen(vm: PulpitViewModel, onConnected: () -> Unit) {
    val cfg by vm.config.collectAsState()
    val conn by vm.connState.collectAsState()

    var host by remember(cfg.host) { mutableStateOf(cfg.host) }
    var port by remember(cfg.port) { mutableStateOf(cfg.port.toString()) }
    var name by remember(cfg.name) { mutableStateOf(cfg.name) }
    var pairCode by remember { mutableStateOf("") }
    val paired = !cfg.token.isNullOrBlank()

    Column(
        Modifier
            .fillMaxSize()
            .background(DeckColors.background)
            .padding(24.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(
            text = "Pulpit",
            style = MaterialTheme.typography.headlineMedium,
            color = Color.White,
        )
        Text(
            text = "connect to your desktop server",
            style = MaterialTheme.typography.bodySmall,
            color = Color.White.copy(alpha = 0.6f),
            modifier = Modifier.padding(bottom = 24.dp),
        )

        OutlinedTextField(
            value = host,
            onValueChange = { host = it },
            label = { Text("PC IP address") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Row(Modifier.fillMaxWidth().padding(top = 8.dp)) {
            OutlinedTextField(
                value = port,
                onValueChange = { port = it.filter(Char::isDigit).take(5) },
                label = { Text("Port") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                modifier = Modifier.weight(1f),
            )
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                label = { Text("Device name") },
                singleLine = true,
                modifier = Modifier.weight(1.4f).padding(start = 8.dp),
            )
        }

        if (paired) {
            Text(
                text = "paired - Connect uses the stored device token",
                color = Color.White.copy(alpha = 0.6f),
                modifier = Modifier.padding(top = 12.dp),
            )
            TextButton(
                text = "Forget pairing",
                onClick = { vm.forgetPairing() },
            )
        } else {
            OutlinedTextField(
                value = pairCode,
                onValueChange = { pairCode = it.uppercase().filter { c -> c.isLetterOrDigit() }.take(8) },
                label = { Text("Pairing code (desktop: \"Dodaj urządzenie\")") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Text),
                modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
            )
        }

        when (conn) {
            is ConnState.Connecting -> {
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    modifier = Modifier.padding(16.dp),
                ) {
                    CircularProgressIndicator()
                    Text(
                        text = "Connecting to ${cfg.host}:${cfg.port}...",
                        color = Color.White.copy(alpha = 0.7f),
                        style = MaterialTheme.typography.bodySmall,
                        modifier = Modifier.padding(top = 10.dp),
                    )
                }
            }
            is ConnState.Failed -> {
                Text(
                    text = (conn as ConnState.Failed).reason,
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.padding(8.dp),
                )
            }
            else -> {}
        }

        Button(
            onClick = {
                vm.saveConfig(
                    cfg.copy(host = host.trim(), port = port.toIntOrNull() ?: 8500, name = name.trim()),
                )
                if (paired) {
                    vm.connect()
                } else {
                    vm.connectWithPairCode(pairCode)
                }
                onConnected()
            },
            modifier = Modifier.padding(top = 16.dp).fillMaxWidth(),
        ) {
            Text(if (paired) "Connect" else "Pair")
        }
    }
}

@Composable
private fun TextButton(text: String, onClick: () -> Unit) {
    androidx.compose.material3.TextButton(onClick = onClick) {
        Text(text, color = Color.White.copy(alpha = 0.7f))
    }
}
