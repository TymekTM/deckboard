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
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.draw.clip
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import app.pulpit.mobile.net.ConnState
import app.pulpit.mobile.net.DiscoveredDesktop
import app.pulpit.mobile.net.NsdDiscovery
import app.pulpit.mobile.state.PulpitViewModel

@Composable
fun ConnectScreen(vm: PulpitViewModel) {
    val cfg by vm.config.collectAsState()
    val conn by vm.connState.collectAsState()
    val context = LocalContext.current

    var host by remember(cfg.host) { mutableStateOf(cfg.host) }
    var port by remember(cfg.port) { mutableStateOf(cfg.port.toString()) }
    var name by remember(cfg.name) { mutableStateOf(cfg.name) }
    var pairCode by remember { mutableStateOf("") }
    // MOB-01: set when the typed host/port cannot become a URL - shown
    // instead of connecting (and before saveConfig persists the bad pair)
    var addressError by remember { mutableStateOf<String?>(null) }
    val paired = !cfg.token.isNullOrBlank()

    // M8 pairing modes: Auto (pick a desktop discovered over mDNS, both
    // screens compare the verification code) and Manual (type the code
    // the desktop minted) - the original flow stays for when discovery
    // cannot see the desktop (firewall, other subnet).
    var autoMode by remember { mutableStateOf(true) }
    val discovered = remember { mutableStateListOf<DiscoveredDesktop>() }
    val pairReq by vm.pairRequest.collectAsState()
    DisposableEffect(autoMode, paired) {
        val nsd = if (autoMode && !paired) {
            NsdDiscovery(context) { found ->
                if (discovered.none { it.host == found.host && it.port == found.port }) {
                    discovered.add(found)
                }
            }.also { it.start() }
        } else {
            null
        }
        onDispose { nsd?.stop() }
    }

    val hapticConfig by vm.hapticConfig.collectAsState()
    val scrollState = rememberScrollState()

    Column(
        Modifier
            .fillMaxSize()
            .background(DeckColors.background)
            .verticalScroll(scrollState)
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
            // The keep-alive service needs to escape Doze on aggressive
            // ROMs; stock Android keeps foreground services running, but
            // the OEM grid (Lineage included) may not. One tap, once.
            val pm = context.getSystemService(PowerManager::class.java)
            val pkg = context.packageName
            if (pm != null && !pm.isIgnoringBatteryOptimizations(pkg)) {
                TextButton(
                    text = "Zezwól na pracę w tle (omijaj oszczędzanie baterii)",
                    onClick = {
                        runCatching {
                            context.startActivity(
                                Intent(
                                    Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
                                    Uri.parse("package:$pkg"),
                                ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                            )
                        }
                    },
                )
            }
            TextButton(
                text = "Forget pairing",
                onClick = { vm.forgetPairing() },
            )
        } else {
            // M8 pairing modes: Auto discovers desktops on the network and
            // finishes with the shared verification code; Manual keeps the
            // desktop-minted code entry.
            Row(Modifier.fillMaxWidth().padding(top = 12.dp)) {
                Button(
                    onClick = { autoMode = true },
                    colors = if (autoMode) ButtonDefaults.buttonColors() else ButtonDefaults.outlinedButtonColors(),
                    modifier = Modifier.weight(1f),
                ) { Text("Auto (w sieci)") }
                OutlinedButton(
                    onClick = { autoMode = false },
                    modifier = Modifier.weight(1f).padding(start = 8.dp),
                ) { Text("Ręcznie (kod)") }
            }

            val waiting = pairReq
            if (autoMode) {
                if (waiting != null) {
                    // both screens show this number; pairing completes only
                    // when the desktop operator confirms the match
                    Text(
                        text = waiting.code,
                        style = MaterialTheme.typography.displayMedium,
                        color = Color.White,
                        modifier = Modifier.padding(top = 16.dp),
                    )
                    Text(
                        text = "Potwierdź na komputerze, że kody są zgodne...",
                        color = Color.White.copy(alpha = 0.7f),
                        modifier = Modifier.padding(top = 4.dp),
                    )
                    OutlinedButton(
                        onClick = { vm.cancelPairRequest() },
                        modifier = Modifier.padding(top = 8.dp),
                    ) { Text("Przerwij") }
                } else if (discovered.isEmpty()) {
                    Text(
                        text = "Szukam komputerów w sieci... (upewnij się, że Pulpit działa)",
                        color = Color.White.copy(alpha = 0.6f),
                        modifier = Modifier.padding(top = 12.dp),
                    )
                } else {
                    discovered.forEach { desktop ->
                        Button(
                            onClick = {
                                vm.startPairRequest(
                                    desktop.host,
                                    desktop.port,
                                    name.ifBlank { Build.MODEL },
                                )
                            },
                            modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                        ) {
                            Text("${desktop.name}  (${desktop.host}:${desktop.port})")
                        }
                    }
                }
            } else {
                OutlinedTextField(
                    value = pairCode,
                    onValueChange = { pairCode = it.uppercase().filter { c -> c.isLetterOrDigit() }.take(8) },
                    label = { Text("Pairing code (desktop: \"Dodaj urządzenie\")") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Text),
                    modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
                )
                // USB path (ROADMAP M4): with the desktop reachable over adb
                // reverse, the server answers on the phone's own loopback.
                TextButton(
                    text = "Przez USB (adb reverse) - wstaw 127.0.0.1",
                    onClick = { host = "127.0.0.1" },
                )
                Text(
                    text = "na PC: adb reverse tcp:8500 tcp:8500",
                    style = MaterialTheme.typography.bodySmall,
                    color = Color.White.copy(alpha = 0.5f),
                    modifier = Modifier.padding(top = 4.dp),
                )
            }
        }

        when (conn) {
            is ConnState.Connecting -> {
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    modifier = Modifier.padding(16.dp),
                ) {
                    // the same fixed-arc ring as the board overlay: the
                    // deck's animator scale is off, a Material
                    // indeterminate spinner would freeze into a dot
                    RingSpinner()
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

        addressError?.let {
            Text(
                text = it,
                color = MaterialTheme.colorScheme.error,
                modifier = Modifier.padding(top = 8.dp),
            )
        }

        HapticsSettings(
            config = hapticConfig,
            onEnabledChange = vm::setHapticsEnabled,
            onIntensityChange = vm::setHapticIntensity,
            modifier = Modifier.padding(top = 16.dp),
        )

        if (!paired && autoMode) {
            // the auto flow is self-contained: picking a desktop starts
            // the request and approval connects on its own
        } else {
            Button(
                onClick = {
                    // MOB-01: validate before saving - a persisted
                    // malformed host crashed (and re-crashed) the app
                    // on every paired launch
                    val portNum = port.toIntOrNull() ?: 0
                    val err = app.pulpit.mobile.net.addressError(host.trim(), portNum)
                    if (err != null) {
                        addressError = err
                        return@Button
                    }
                    addressError = null
                    vm.saveConfig(
                        cfg.copy(host = host.trim(), port = portNum, name = name.trim()),
                    )
                    if (paired) {
                        vm.connect()
                    } else {
                        vm.connectWithPairCode(pairCode)
                    }
                },
                modifier = Modifier.padding(top = 16.dp).fillMaxWidth(),
            ) {
                Text(if (paired) "Connect" else "Pair")
            }
        }
    }
}

@Composable
private fun TextButton(text: String, onClick: () -> Unit) {
    androidx.compose.material3.TextButton(onClick = onClick) {
        Text(text, color = Color.White.copy(alpha = 0.7f))
    }
}

@Composable
fun HapticsSettings(
    config: HapticConfig,
    onEnabledChange: (Boolean) -> Unit,
    onIntensityChange: (HapticIntensity) -> Unit,
    modifier: Modifier = Modifier,
) {
    val haptics = LocalHaptics.current
    Column(
        modifier = modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(Color.White.copy(alpha = 0.05f))
            .padding(12.dp),
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Column {
                Text(
                    text = "Wibracje",
                    style = MaterialTheme.typography.titleSmall,
                    color = Color.White,
                )
                Text(
                    text = if (config.enabled) "Włączone (${config.intensity.label})" else "Wyłączone",
                    style = MaterialTheme.typography.bodySmall,
                    color = Color.White.copy(alpha = 0.6f),
                )
            }
            Switch(
                checked = config.enabled,
                onCheckedChange = {
                    onEnabledChange(it)
                    if (it) haptics.click()
                },
            )
        }

        if (config.enabled) {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                HapticIntensity.values().forEach { intensity ->
                    val selected = config.intensity == intensity
                    if (selected) {
                        Button(
                            onClick = {
                                onIntensityChange(intensity)
                                haptics.click()
                            },
                            modifier = Modifier.weight(1f),
                        ) {
                            Text(intensity.label, style = MaterialTheme.typography.labelMedium)
                        }
                    } else {
                        OutlinedButton(
                            onClick = {
                                onIntensityChange(intensity)
                                haptics.click()
                            },
                            modifier = Modifier.weight(1f),
                        ) {
                            Text(intensity.label, style = MaterialTheme.typography.labelMedium)
                        }
                    }
                }
            }
        }
    }
}