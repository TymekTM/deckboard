//! App state over protocol v2: connection lifecycle (token or pairing),
//! boards snapshot + live deltas, the current board, and the live channel
//! state (scalars + series). Reconnects with backoff; every (re)connect
//! gets a full snapshot from the server, so the on-disk cache
//! (BoardCache) is display-only and never needs invalidating.

package app.pulpit.mobile.state

import android.app.Application
import android.content.Context
import android.content.Intent
import android.graphics.BitmapFactory
import android.os.Build
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.lifecycle.AndroidViewModel
import app.pulpit.mobile.LinkService
import app.pulpit.mobile.net.ConnState
import app.pulpit.mobile.net.V2Client
import app.pulpit.mobile.net.V2Event
import app.pulpit.mobile.net.createPairRequest
import app.pulpit.mobile.net.pairRequestStatus
import app.pulpit.mobile.proto.Board
import app.pulpit.mobile.proto.ChannelInfo
import app.pulpit.mobile.proto.BoardOp
import app.pulpit.mobile.proto.Tile
import app.pulpit.mobile.proto.V2
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import java.util.concurrent.TimeUnit

data class ServerConfig(
    val host: String,
    val port: Int,
    val name: String,
    val token: String?,
)

class PulpitViewModel(app: Application) : AndroidViewModel(app) {

    /** Every field below is confined to the main thread: Compose calls in
     *  from there, and this scope runs every coroutine there too. Blocking
     *  work hops to Dispatchers.IO explicitly (ensureAsset); frame decoding
     *  already happens on OkHttp's thread inside V2Client. SupervisorJob so
     *  one failed child cannot cancel the reconnect loop and the probe. */
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    private val prefs = app.getSharedPreferences("pulpit", Context.MODE_PRIVATE)

    /** Keystore-backed cipher for the token at rest (ADR-008); the pure
     *  decisions it feeds live in TokenVault (unit-tested). */
    private val tokenCipher = KeystoreTokenCipher()

    private val _config = MutableStateFlow(loadConfig())
    val config: StateFlow<ServerConfig> = _config

    private val _connState = MutableStateFlow<ConnState>(ConnState.Disconnected)
    val connState: StateFlow<ConnState> = _connState

    /** True once the server announced its shutdown: the goodbye overlay is
     *  up, the normal retry loop is suspended, and reconnects happen only
     *  silently (foreground probe) or on a tap. Cleared by any successful
     *  connect or by [reconnectFromShutdown]. */
    private val _serverDown = MutableStateFlow(false)
    val serverDown: StateFlow<Boolean> = _serverDown

    /** True once the link has been down for LINK_STANDBY_MS while the app
     *  was in front: the Activity stops holding the screen on and the
     *  system timeout puts the display to sleep. Cleared by a successful
     *  connect and by the next foreground (the user woke the deck). */
    private val _linkStandby = MutableStateFlow(false)
    val linkStandby: StateFlow<Boolean> = _linkStandby

    private var standbyJob: Job? = null

    /** Tracks the Activity's STARTED/STOPPED: the socket lives only while
     *  the app is in front (see onAppBackground), and retries, the probe,
     *  and the standby countdown run only then. */
    @Volatile private var foreground = false

    /** Reconnect attempts since the last successful session; the banner
     *  shows it so the retry loop is visible instead of mysterious. */
    private val _reconnectAttempt = MutableStateFlow(0)
    val reconnectAttempt: StateFlow<Int> = _reconnectAttempt

    private val _boards = MutableStateFlow<List<Board>>(emptyList())
    val boards: StateFlow<List<Board>> = _boards

    private val _currentBoard = MutableStateFlow<Board?>(null)
    val currentBoard: StateFlow<Board?> = _currentBoard

    private val _deviceName = MutableStateFlow("")
    val deviceName: StateFlow<String> = _deviceName

    /** Last pushed value per state channel (`state.sync` merged with
     *  `state.patch`). */
    private val _values = MutableStateFlow<Map<String, JsonElement>>(emptyMap())
    val values: StateFlow<Map<String, JsonElement>> = _values

    /** Series windows per channel, oldest first (server keeps the history). */
    private val _series = MutableStateFlow<Map<String, List<Double>>>(emptyMap())
    val series: StateFlow<Map<String, List<Double>>> = _series

    /** Decoded tile/board images by asset hash (content-addressed, so the
     *  map is safe across reconnects to any server). */
    private val _bitmaps = MutableStateFlow<Map<String, ImageBitmap>>(emptyMap())
    val bitmaps: StateFlow<Map<String, ImageBitmap>> = _bitmaps

    /** Hashes fetched, in flight, or failed this process (main thread
     *  only). Failures retry ASSET_RETRIES times with backoff; forgetPairing
     *  clears the set together with the bitmaps it guards. */
    private val assetFetches = mutableSetOf<String>()

    private var client: V2Client? = null
    private var eventJob: Job? = null
    private var reconnectAttempts = 0

    /** Display-only offline cache (see [BoardCache]): written on every
     *  full snapshot so a cold launch shows the board while the link
     *  reconnects. Cleared on unpair. */
    private val cache = BoardCache(File(app.filesDir, "cache"))

    /** One pending reconnect per connect cycle: Failed and Disconnected
     *  arrive back to back, and each state flip would otherwise schedule
     *  a duplicate timer (double-counting the attempt budget). */
    private var reconnectJob: Job? = null

    /** Set while a pairing is in flight (no token yet). */
    private var pendingPairCode: String? = null

    /** Channels the server declared as series in the welcome catalog.
     *  Patches for these append to the chart window even when the
     *  connect-time snapshot carried no history yet (fresh server). */
    private var seriesChannels: Set<String> = emptySet()

    /** Welcome channel catalog: shape plus the graph tiles' display
     *  title/suffix captured by the server from pushed values. */
    private val _channelMeta =
        MutableStateFlow<Map<String, ChannelInfo>>(emptyMap())
    val channelMeta: StateFlow<Map<String, ChannelInfo>> = _channelMeta

    // -- M8 discovery pairing (Bluetooth-style, plan 014) -------------------

    /** Live pair-request: the verification code both screens show, plus
     *  where the request went (and the hello name to persist on
     *  approval - the desktop's request log shows the same name). */
    data class PairRequestUi(
        val host: String,
        val port: Int,
        val code: String,
        val deviceName: String,
    )

    private val _pairRequest = MutableStateFlow<PairRequestUi?>(null)
    val pairRequest: StateFlow<PairRequestUi?> = _pairRequest

    /** Asks a discovered desktop to pair. The desktop shows its dialog
     *  with the same code; this polls until the operator decides and
     *  then finishes through the ordinary pairing path (pre-approved
     *  server-side, so no second dialog). */
    fun startPairRequest(host: String, port: Int, deviceName: String) {
        if (_pairRequest.value != null) return
        scope.launch {
            try {
                val created = createPairRequest(sharedHttp, host, port, deviceName)
                _pairRequest.value = PairRequestUi(host, port, created.code, deviceName)
                pollPairDecision(host, port, created.request_id, created.expires_in_secs)
            } catch (e: Exception) {
                _pairRequest.value = null
                _connState.value = ConnState.Failed(
                    e.message ?: "żądanie parowania nie powiodło się",
                    retryable = false,
                )
            }
        }
    }

    /** Leaves the waiting state without touching the desktop (its dialog
     *  still resolves on its own; the unanswered request expires). */
    fun cancelPairRequest() {
        _pairRequest.value = null
    }

    private suspend fun pollPairDecision(host: String, port: Int, id: String, ttlSecs: Long) {
        val deadline = System.currentTimeMillis() + (ttlSecs + 5) * 1000
        while (System.currentTimeMillis() < deadline) {
            delay(2000)
            val status = try {
                pairRequestStatus(sharedHttp, host, port, id)
            } catch (_: Exception) {
                continue
            }
            when (status) {
                "approved" -> {
                    val ui = _pairRequest.value
                    _pairRequest.value = null
                    if (ui != null) {
                        withContext(Dispatchers.Main.immediate) {
                            saveConfig(
                                _config.value.copy(
                                    host = ui.host,
                                    port = ui.port,
                                    name = ui.deviceName,
                                ),
                            )
                            connectWithPairCode(ui.code)
                        }
                    }
                    return
                }
                "rejected" -> {
                    _pairRequest.value = null
                    _connState.value =
                        ConnState.Failed("komputer odrzucił parowanie", retryable = false)
                    return
                }
                "expired" -> {
                    _pairRequest.value = null
                    _connState.value = ConnState.Failed(
                        "żądanie wygasło - uruchom parowanie ponownie",
                        retryable = false,
                    )
                    return
                }
            }
        }
        if (_pairRequest.value != null) {
            _pairRequest.value = null
            _connState.value =
                ConnState.Failed("komputer nie odpowiedział w czasie", retryable = false)
        }
    }

    init {
        // A paired device reconnects on its own; pairing needs the user
        // to enter a fresh code.
        if (!_config.value.token.isNullOrBlank()) {
            loadCacheIntoUi()
            connect()
            startKeepAlive()
        }
        // The silent probe: while the shutdown overlay is up and the app is
        // foreground, poke the server every PROBE_SECONDS so a restarted
        // machine picks the deck back up without a tap. Background = dark
        // screen = no attempts at all.
        scope.launch {
            while (true) {
                delay(PROBE_SECONDS * 1000L)
                if (foreground && _serverDown.value) {
                    Plog.i(TAG, "shutdown probe: trying the server again")
                    connect()
                }
            }
        }
    }

    override fun onCleared() {
        disconnect()
        scope.cancel()
    }

    // -- offline cache (display-only) --------------------------------------

    /** Cold launch with a stored pairing: paint the last snapshot before
     *  the first bytes hit the wire, so the deck comes up looking alive
     *  and the retry banner says how the link really is. */
    private fun loadCacheIntoUi() {
        val snap = cache.load() ?: return
        Plog.i(TAG, "offline cache: ${snap.boards.size} boards restored")
        _boards.value = snap.boards.sortedBy { it.order }
        _currentBoard.value = _boards.value.firstOrNull()
        _values.value = snap.values
        _series.value = snap.series
    }

    /** Fire-and-forget persist of the latest full snapshot (boards or
     *  state.sync); patches are not persisted - the next snapshot will
     *  be, and a cold launch only needs the shape of the deck. */
    private fun persistCache() {
        if (_boards.value.isEmpty()) return
        val payload = CachePayload(_boards.value, _values.value, _series.value)
        scope.launch(Dispatchers.IO) { cache.save(payload) }
    }

    // -- keep-alive (foreground service) ------------------------------------

    /** Raise [LinkService] so Doze cannot starve the link once the
     *  screen goes dark (ROADMAP M4). The service holds no socket; it
     *  only keeps the process at foreground priority and flips the flag
     *  [closesLinkOnBackground] consults. */
    private fun startKeepAlive() {
        val app = getApplication<Application>()
        LinkBus.status.value = linkStatusLine(_connState.value, _config.value.host)
        runCatching {
            val intent = Intent(app, LinkService::class.java)
            if (Build.VERSION.SDK_INT >= 26) {
                app.startForegroundService(intent)
            } else {
                app.startService(intent)
            }
        }.onFailure { Plog.w(TAG, "keep-alive service refused to start: ${it.message}") }
    }

    /** Back to plan 008 (close on background): unpair, or the user hit
     *  "Rozłącz" and the service stopped itself. */
    private fun stopKeepAlive() {
        val app = getApplication<Application>()
        runCatching { app.stopService(Intent(app, LinkService::class.java)) }
    }

    /** Kick off a fetch for [hash]. Reads [ServerConfig.token], so nothing
     *  loads before the device is authenticated. Failures back off and
     *  retry a few times - e.g. an asset fetched during a server restart. */
    fun ensureAsset(hash: String, attempt: Int = 0) {
        if (_bitmaps.value.containsKey(hash) || !assetFetches.add(hash)) return
        val cfg = _config.value
        val token = cfg.token ?: return
        scope.launch {
            val bitmap = withContext(Dispatchers.IO) {
                runCatching {
                    val url = "http://${cfg.host}:${cfg.port}/assets/$hash?token=$token"
                    sharedHttp.newCall(Request.Builder().url(url).build()).execute().use { resp ->
                        if (!resp.isSuccessful) return@use null
                        // one read: OkHttp streams cannot be consumed twice
                        val bytes = resp.body?.byteStream()?.readBytes() ?: return@use null
                        // a tile renders ~150px; decode with a power-of-two
                        // sample so a future full-res photo cannot eat the
                        // heap of a 1 GB tablet
                        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
                        val sampled = BitmapFactory.Options().apply {
                            inSampleSize = maxOf(
                                bounds.outWidth / ASSET_MAX_DIM,
                                bounds.outHeight / ASSET_MAX_DIM,
                                1,
                            )
                        }
                        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, sampled)
                    }
                }.getOrNull()
            }
            if (bitmap != null) {
                _bitmaps.value = _bitmaps.value + (hash to bitmap.asImageBitmap())
            } else if (attempt < ASSET_RETRIES) {
                delay(30_000L * (attempt + 1))
                assetFetches.remove(hash)
                ensureAsset(hash, attempt + 1)
            }
        }
    }

    /** Prefs → [ServerConfig]. The stored token is either an `enc1:`
     *  envelope (decrypted) or a legacy plaintext from before ADR-008
     *  (kept, then re-encrypted on this load); anything unreadable reads
     *  as unpaired - the device re-pairs, nothing crashes. Keystore work
     *  happens only on a token read/write, once per launch at most.
     *  allowBackup=false keeps the file out of cloud backups, and a
     *  backup would be useless anyway: the AndroidKeyStore key is
     *  non-exportable and bound to this device+user, so restored
     *  ciphertext could never be decrypted elsewhere - a restored
     *  install simply re-pairs. */
    private fun loadConfig(): ServerConfig {
        val load = planTokenLoad(prefs.getString("token", null)) { iv, data ->
            tokenCipher.decrypt(iv, data)
        }
        if (load.rewrite) {
            if (load.token == null) {
                Plog.i(TAG, "stored pairing token unreadable - the device must pair again")
            } else {
                Plog.i(TAG, "migrating plaintext pairing token to keystore encryption")
            }
            storeToken(load.token)
        }
        return ServerConfig(
            host = prefs.getString("host", "") ?: "",
            port = prefs.getInt("port", 8500),
            name = prefs.getString("name", "") ?: "",
            token = load.token,
        )
    }

    /** The only place the token reaches disk: encrypted, or removed.
     *  When encryption fails (broken keystore) nothing is persisted -
     *  plaintext at rest is what ADR-008 forbids - and the caller's
     *  in-memory token keeps the current session alive; the device
     *  re-pairs after a restart. Never logs token material. */
    private fun storeToken(token: String?) {
        val editor = prefs.edit()
        val envelope = token?.let { t ->
            tokenCipher.encrypt(t)?.let { (iv, data) -> encodeEnvelope(iv, data) }
        }
        if (token != null && envelope == null) {
            // encryption failed (unpair with a null token is the normal
            // path): the reason is worth a line, the token never is.
            Plog.w(TAG, "token vault: encrypt failed - token not persisted, re-pair after restart")
        }
        if (envelope == null) {
            editor.remove("token")
        } else {
            editor.putString("token", envelope)
        }
        editor.apply()
    }

    fun saveConfig(cfg: ServerConfig) {
        storeToken(cfg.token)
        prefs.edit()
            .putString("host", cfg.host)
            .putInt("port", cfg.port)
            .putString("name", cfg.name)
            .apply()
        _config.value = cfg
    }

    /** Connect with the stored token. */
    fun connect() {
        val token = _config.value.token
        if (token.isNullOrBlank()) {
            _connState.value = ConnState.Failed("device not paired - enter a pairing code")
            return
        }
        openClient(token = token, pairCode = null)
    }

    /** First-time pairing: the desktop shows a one-time code (POST /v2/pair
     *  output). On success the issued token is stored. */
    fun connectWithPairCode(code: String) {
        pendingPairCode = code.trim().uppercase()
        openClient(token = null, pairCode = pendingPairCode)
    }

    private fun openClient(token: String?, pairCode: String?) {
        disconnect()
        val cfg = _config.value
        val c = V2Client(cfg.host, cfg.port, token, pairCode, cfg.name)
        client = c
        observeEvents(c)
        c.connect()
    }

    fun disconnect() {
        reconnectJob?.cancel()
        reconnectJob = null
        eventJob?.cancel()
        client?.disconnect()
        client = null
        pendingPairCode = null
        _connState.value = ConnState.Disconnected
        // The last board stays on screen (dimmed by the status banner):
        // every reconnect resyncs from scratch, so the snapshot cannot go
        // stale in any way the protocol does not overwrite immediately.
    }

    fun forgetPairing() {
        saveConfig(_config.value.copy(token = null))
        stopKeepAlive()
        disconnect()
        cache.clear()
        _boards.value = emptyList()
        _currentBoard.value = null
        _values.value = emptyMap()
        _series.value = emptyMap()
        _channelMeta.value = emptyMap()
        _bitmaps.value = emptyMap()
        assetFetches.clear()
    }

    /** The user tapped the shutdown overlay: leave the standby state and
     *  retry immediately, with the normal reconnecting banner visible. */
    fun reconnectFromShutdown() {
        _serverDown.value = false
        reconnectAttempts = 1
        _reconnectAttempt.value = 1
        connect()
    }

    fun onAppForeground() {
        foreground = true
        // The user just woke the deck: hold the screen again and look for
        // the PC right away instead of waiting out a backoff.
        _linkStandby.value = false
        if (!_config.value.token.isNullOrBlank() && reconnectJob?.isActive != true) {
            val st = _connState.value
            val idle = client == null || st is ConnState.Disconnected ||
                (st is ConnState.Failed && st.retryable)
            if (idle) {
                // In shutdown standby this is the silent probe: the
                // goodbye screen stays until a welcome clears it.
                if (!_serverDown.value) {
                    reconnectAttempts = 0
                    _reconnectAttempt.value = 0
                }
                connect()
            }
        }
        if (_connState.value !is ConnState.Connected) armStandby()
    }

    fun onAppBackground() {
        foreground = false
        standbyJob?.cancel()
        standbyJob = null
        // A refusal already closed its socket and its message must stay
        // on the connect screen; a pairing in flight cannot be retried
        // (one-time code). Leave both alone.
        val refused = (_connState.value as? ConnState.Failed)?.retryable == false
        if (refused || pendingPairCode != null) return
        // 008's battery win, with the M4 keep-alive escape hatch: while
        // the foreground service is up the deck stays connected in the
        // dark; otherwise the socket closes so the radio and the CPU can
        // sleep. Every connect gets a full snapshot, so nothing goes
        // stale; onAppForeground reconnects.
        if (!closesLinkOnBackground(
                LinkBus.keepLinkInBackground.value,
                refused,
                pendingPairCode != null,
            )
        ) {
            Plog.i(TAG, "app in background - keep-alive service holds the link")
            return
        }
        Plog.i(TAG, "app in background - closing the link")
        disconnect()
    }

    private fun observeEvents(client: V2Client) {
        eventJob = scope.launch {
            launch {
                client.state.collect { st ->
                    _connState.value = st
                    // the keep-alive notification mirrors the link line
                    LinkBus.status.value = linkStatusLine(st, _config.value.host)
                    when (st) {
                        is ConnState.Connected -> {
                            reconnectAttempts = 0
                            _reconnectAttempt.value = 0
                            _serverDown.value = false
                            disarmStandby()
                        }
                        is ConnState.ServerDown -> {
                            Plog.i(TAG, "server announced shutdown - retry loop suspended")
                            _serverDown.value = true
                        }
                        // A refusal is final: stay on the connect screen with the reason.
                        is ConnState.Failed -> {
                            armStandby()
                            when {
                                !st.retryable -> pendingPairCode = null
                                !_serverDown.value -> scheduleReconnect()
                            }
                        }
                        // While the overlay is up the probe owns reconnects:
                        // failures are expected and stay invisible.
                        is ConnState.Disconnected -> {
                            armStandby()
                            if (!_serverDown.value) scheduleReconnect()
                        }
                        is ConnState.Connecting -> armStandby()
                    }
                }
            }
            launch {
                // The channel buffers every frame the socket delivered, so
                // the sync burst right after hello is never lost to a slow
                // subscription.
                for (ev in client.events) {
                    handleEvent(client, ev)
                }
            }
        }
    }

    private fun handleEvent(client: V2Client, ev: V2Event) {
        when (ev) {
            is V2Event.WelcomeReady -> {
                seriesChannels = ev.welcome.channels
                    .filterValues { it.shape == V2.SHAPE_SERIES }
                    .keys
                _channelMeta.value = ev.welcome.channels
                ev.issuedToken?.let { token ->
                    Plog.i(TAG, "paired, storing device token")
                    saveConfig(_config.value.copy(token = token))
                    // first pairing: raise the keep-alive service too
                    startKeepAlive()
                }
                _deviceName.value = ev.welcome.device.name
                pendingPairCode = null
            }
            is V2Event.Boards -> {
                _boards.value = ev.boards.sortedBy { it.order }
                val cur = _currentBoard.value
                _currentBoard.value = _boards.value.firstOrNull { it.id == cur?.id }
                    ?: _boards.value.firstOrNull()
                persistCache()
            }
            is V2Event.Delta -> applyDelta(ev.ops)
            is V2Event.SwitchBoard -> {
                _currentBoard.value = _boards.value.firstOrNull { it.id == ev.boardId }
                    ?: _currentBoard.value
            }
            is V2Event.State -> {
                _values.value = ev.values
                _series.value = ev.series
                persistCache()
            }
            is V2Event.Patch -> {
                val values = _values.value.toMutableMap()
                val series = _series.value.toMutableMap()
                for (change in ev.changes) {
                    val info = change.value
                    // series channels carry their newest point; scalars replace
                    if (change.channel in seriesChannels) {
                        val point = (info as? kotlinx.serialization.json.JsonPrimitive)
                            ?.content?.toDoubleOrNull()
                        if (point != null) {
                            // the window may not exist yet: a fresh server
                            // sends an empty state.sync and only patches
                            // from here on build the chart history
                            val window = ((series[change.channel] ?: emptyList()) + point)
                                .takeLast(V2.SERIES_CAP)
                            series[change.channel] = window
                        }
                    }
                    values[change.channel] = info
                }
                _values.value = values
                _series.value = series
            }
            is V2Event.ServerError -> {
                // Fatal codes are already terminal in V2Client
                // (ConnState.Failed, retryable = false); nothing to do here.
                Plog.w(TAG, "server error: ${ev.code} ${ev.message.orEmpty()}")
            }
            is V2Event.Acked -> {} // interactions are fire-and-confirm
        }
    }

    /** Deltas mutate the snapshot the server already sent (docs/protocol-v2.md §4). */
    private fun applyDelta(ops: List<BoardOp>) {
        _boards.value = applyBoardOps(_boards.value, ops)
        val cur = _currentBoard.value
        val currentId = cur?.id
        if (currentId != null && _boards.value.none { it.id == currentId }) {
            _currentBoard.value = _boards.value.firstOrNull()
        } else if (currentId != null) {
            // refresh the selected board object so tile edits show up
            _currentBoard.value = _boards.value.firstOrNull { it.id == currentId }
        }
    }

    private fun scheduleReconnect() {
        // Pairing codes are one-time: a dropped pairing socket cannot be
        // retried with the same code, so only paired devices reconnect.
        val token = _config.value.token
        if (token.isNullOrBlank()) {
            if (pendingPairCode != null) {
                pendingPairCode = null
                _connState.value = ConnState.Failed("pairing failed - generate a new code on the desktop")
            }
            return
        }
        if (reconnectJob?.isActive == true) return
        reconnectAttempts++
        _reconnectAttempt.value = reconnectAttempts
        reconnectJob = scope.launch {
            delay(reconnectAttempts.coerceAtMost(6) * 2_000L)
            reconnectJob = null
            // A retry scheduled just before the goodbye arrived must not
            // fire into standby; the probe owns reconnecting from there.
            // Nor may it fire into the background: the socket is closed.
            if (_serverDown.value || !foreground) return@launch
            val st = _connState.value
            if ((st is ConnState.Failed && st.retryable) || st is ConnState.Disconnected) {
                Plog.i(TAG, "reconnect attempt $reconnectAttempts")
                connect()
            }
        }
    }

    /** Start the standby countdown on the first non-connected state while
     *  in front. Idempotent: reconnect attempts do not restart it, so the
     *  3 minutes count from the moment the link was lost. */
    private fun armStandby() {
        if (!foreground || _linkStandby.value || standbyJob?.isActive == true) return
        standbyJob = scope.launch {
            delay(LINK_STANDBY_MS)
            standbyJob = null
            Plog.i(TAG, "link down for ${LINK_STANDBY_MS / 60_000} min - letting the screen sleep")
            _linkStandby.value = true
        }
    }

    private fun disarmStandby() {
        standbyJob?.cancel()
        standbyJob = null
        _linkStandby.value = false
    }

    // -- user interactions ------------------------------------------------
    // Clients send only gestures the tile declares (docs/protocol-v2.md §6).

    fun pressStart(boardId: Long, tile: Tile) {
        if (tile.interacts(V2.INT_PRESS_START)) {
            client?.pressStart(boardId, tile.id)
        }
    }

    fun pressEnd(boardId: Long, tile: Tile) {
        when {
            tile.interacts(V2.INT_PRESS_END) -> client?.pressEnd(boardId, tile.id)
            tile.interacts(V2.INT_TAP) -> client?.tap(boardId, tile.id)
        }
    }

    fun slider(boardId: Long, tile: Tile, value: Float) {
        if (tile.interacts(V2.INT_SLIDE)) {
            client?.slide(boardId, tile.id, value)
        }
    }

    /** M5 custom gestures: sent only for interactions the tile declares
     *  (the server enforces the same list, so an undeclared gesture is a
     *  guaranteed typed error - do not send it). */
    fun gesture(boardId: Long, tile: Tile, name: String) {
        if (tile.interacts(name)) {
            client?.gesture(boardId, tile.id, name)
        }
    }

    fun selectBoard(board: Board) {
        _currentBoard.value = board
    }

    companion object {
        private const val TAG = "PulpitViewModel"

        private const val ASSET_RETRIES = 3

        /** Shutdown-overlay probe cadence; see the init loop. */
        private const val PROBE_SECONDS = 30L

        /** Decode cap for tile images: tiles render around 150px, so a
         *  512px sample is plenty even on a 2x2-tile widget. */
        private const val ASSET_MAX_DIM = 512

        /** Shared by reconnects and asset fetches - see V2Client.http. */
        private val sharedHttp = OkHttpClient.Builder()
            .connectTimeout(6, TimeUnit.SECONDS)
            .build()
    }
}
