//! App state over protocol v2: connection lifecycle (token or pairing),
//! boards snapshot + live deltas, the current board, and the live channel
//! state (scalars + series). Reconnects with backoff; every (re)connect
//! gets a full snapshot from the server, so there is no client cache to
//! invalidate.

package app.deckboard.mobile.state

import android.app.Application
import android.content.Context
import android.graphics.BitmapFactory
import android.util.Log
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.lifecycle.AndroidViewModel
import app.deckboard.mobile.net.ConnState
import app.deckboard.mobile.net.V2Client
import app.deckboard.mobile.net.V2Event
import app.deckboard.mobile.proto.Board
import app.deckboard.mobile.proto.BoardOp
import app.deckboard.mobile.proto.Tile
import app.deckboard.mobile.proto.V2
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import okhttp3.OkHttpClient
import okhttp3.Request
import java.util.concurrent.TimeUnit

data class ServerConfig(
    val host: String,
    val port: Int,
    val name: String,
    val token: String?,
)

class DeckboardViewModel(app: Application) : AndroidViewModel(app) {

    private val scope = CoroutineScope(Job())

    private val prefs = app.getSharedPreferences("deckboard", Context.MODE_PRIVATE)

    private val _config = MutableStateFlow(
        ServerConfig(
            host = prefs.getString("host", "") ?: "",
            port = prefs.getInt("port", 8500),
            name = prefs.getString("name", "") ?: "",
            token = prefs.getString("token", null),
        ),
    )
    val config: StateFlow<ServerConfig> = _config

    private val _connState = MutableStateFlow<ConnState>(ConnState.Disconnected)
    val connState: StateFlow<ConnState> = _connState

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

    /** Hashes with a fetch in flight or failed this process; failures are
     *  not retried - a 404 stays a 404 until the app restarts. */
    private val assetFetches = mutableSetOf<String>()

    private var client: V2Client? = null
    private var eventJob: Job? = null
    private var reconnectAttempts = 0

    /** Set while a pairing is in flight (no token yet). */
    private var pendingPairCode: String? = null

    init {
        // A paired device reconnects on its own; pairing needs the user
        // to enter a fresh code.
        if (!_config.value.token.isNullOrBlank()) {
            connect()
        }
    }

    override fun onCleared() {
        disconnect()
        scope.cancel()
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

    fun saveConfig(cfg: ServerConfig) {
        prefs.edit()
            .putString("host", cfg.host)
            .putInt("port", cfg.port)
            .putString("name", cfg.name)
            .putString("token", cfg.token)
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
        eventJob?.cancel()
        client?.disconnect()
        client = null
        pendingPairCode = null
        _connState.value = ConnState.Disconnected
        _boards.value = emptyList()
        _currentBoard.value = null
        _values.value = emptyMap()
        _series.value = emptyMap()
    }

    fun forgetPairing() {
        saveConfig(_config.value.copy(token = null))
        disconnect()
    }

    private fun observeEvents(client: V2Client) {
        eventJob = scope.launch {
            launch {
                client.state.collect { st ->
                    _connState.value = st
                    when (st) {
                        is ConnState.Connected -> reconnectAttempts = 0
                        is ConnState.Failed, is ConnState.Disconnected -> scheduleReconnect()
                        else -> {}
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
                ev.issuedToken?.let { token ->
                    Log.i(TAG, "paired, storing device token")
                    saveConfig(_config.value.copy(token = token))
                }
                _deviceName.value = ev.welcome.device.name
                pendingPairCode = null
            }
            is V2Event.Boards -> {
                _boards.value = ev.boards.sortedBy { it.order }
                val cur = _currentBoard.value
                _currentBoard.value = _boards.value.firstOrNull { it.id == cur?.id }
                    ?: _boards.value.firstOrNull()
            }
            is V2Event.Delta -> applyDelta(ev.ops)
            is V2Event.SwitchBoard -> {
                _currentBoard.value = _boards.value.firstOrNull { it.id == ev.boardId }
                    ?: _currentBoard.value
            }
            is V2Event.State -> {
                _values.value = ev.values
                _series.value = ev.series
            }
            is V2Event.Patch -> {
                val values = _values.value.toMutableMap()
                val series = _series.value.toMutableMap()
                for (change in ev.changes) {
                    val info = change.value
                    // series channels carry their newest point; scalars replace
                    if (series.containsKey(change.channel)) {
                        val point = (info as? kotlinx.serialization.json.JsonPrimitive)
                            ?.content?.toDoubleOrNull()
                        if (point != null) {
                            // mirror the server's ring cap so a chatty
                            // channel cannot grow the window unbounded
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
                Log.w(TAG, "server error: ${ev.code} ${ev.message.orEmpty()}")
                if (ev.code == "pair-invalid" || ev.code == "pair-expired" || ev.code == "unauthorized") {
                    // Bad auth: stop retrying; the connect screen explains.
                    client.disconnect()
                    _connState.value = ConnState.Failed(authMessage(ev.code))
                }
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
        reconnectAttempts++
        scope.launch {
            delay(reconnectAttempts.coerceAtMost(6) * 2_000L)
            val st = _connState.value
            if (st is ConnState.Failed || st is ConnState.Disconnected) {
                Log.i(TAG, "reconnect attempt $reconnectAttempts")
                connect()
            }
        }
    }

    private fun authMessage(code: String): String = when (code) {
        "pair-invalid" -> "invalid pairing code - generate a new one on the desktop"
        "pair-expired" -> "pairing code expired - generate a new one on the desktop"
        else -> "device revoked on the desktop - pair again"
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

    fun selectBoard(board: Board) {
        _currentBoard.value = board
    }

    companion object {
        private const val TAG = "DeckboardViewModel"

        private const val ASSET_RETRIES = 3

        /** Decode cap for tile images: tiles render around 150px, so a
         *  512px sample is plenty even on a 2x2-tile widget. */
        private const val ASSET_MAX_DIM = 512

        /** Shared by reconnects and asset fetches - see V2Client.http. */
        private val sharedHttp = OkHttpClient.Builder()
            .connectTimeout(6, TimeUnit.SECONDS)
            .build()
    }
}
