//! App state: connection lifecycle, boards snapshot, current board, live
//! custom values. Reconnects with backoff; every (re)connect pulls a full
//! `get_shortcuts` snapshot (the reconnect rule from ADR-006).

package app.deckboard.mobile.state

import android.app.Application
import android.content.Context
import android.util.Log
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import app.deckboard.mobile.net.ConnState
import app.deckboard.mobile.net.DeckEvent
import app.deckboard.mobile.net.DeckboardClient
import app.deckboard.mobile.proto.Board
import app.deckboard.mobile.proto.Shortcut
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

data class ServerConfig(val host: String, val port: Int, val accessKey: String)

class DeckboardViewModel(app: Application) : AndroidViewModel(app) {

    private val json = Json { ignoreUnknownKeys = true; isLenient = true }

    private val prefs = app.getSharedPreferences("deckboard", Context.MODE_PRIVATE)

    private val _config = MutableStateFlow(
        ServerConfig(
            host = prefs.getString("host", "") ?: "",
            port = prefs.getInt("port", 8500),
            accessKey = prefs.getString("accessKey", PRO_ACCESS_KEY) ?: PRO_ACCESS_KEY,
        ),
    )
    val config: StateFlow<ServerConfig> = _config

    private val _connState = MutableStateFlow<ConnState>(ConnState.Disconnected)
    val connState: StateFlow<ConnState> = _connState

    private val _boards = MutableStateFlow<List<Board>>(emptyList())
    val boards: StateFlow<List<Board>> = _boards

    private val _currentBoard = MutableStateFlow<Board?>(null)
    val currentBoard: StateFlow<Board?> = _currentBoard

    /** Merged custom-value state (`app_status_update` data per key). */
    private val _customValues = MutableStateFlow<Map<String, JsonElement>>(emptyMap())
    val customValues: StateFlow<Map<String, JsonElement>> = _customValues

    /** One parsed `value`/`suffix` label per watch key, computed once per
     * push so tiles render without re-parsing JSON on recomposition. */
    data class LiveScalar(val text: String?, val suffix: String?)

    private val _liveScalars = MutableStateFlow<Map<String, LiveScalar>>(emptyMap())
    val liveScalars: StateFlow<Map<String, LiveScalar>> = _liveScalars

    /** Value series per key, mirroring the original `setCustomValues`:
     * object payloads with a `value` field append to a history capped at
     * 10 entries; scalars replace in place. */
    private val _valueHistory = MutableStateFlow<Map<String, List<Float>>>(emptyMap())
    val valueHistory: StateFlow<Map<String, List<Float>>> = _valueHistory

    private val _serverVersion = MutableStateFlow("")
    val serverVersion: StateFlow<String> = _serverVersion

    private var client: DeckboardClient? = null
    private var eventJob: Job? = null
    private var reconnectAttempts = 0

    /** Bumped by every connect(); a pending reconnect from an older cycle
     * no-ops instead of racing the fresh connection. */
    private var connectGeneration = 0

    /** Cycle that currently has a reconnect delay pending; Failed and
     * Disconnected often fire for the same failure, and each must not
     * burn its own attempt. */
    private var pendingReconnectGeneration = -1

    fun saveConfig(cfg: ServerConfig) {
        prefs.edit()
            .putString("host", cfg.host)
            .putInt("port", cfg.port)
            .putString("accessKey", cfg.accessKey)
            .apply()
        _config.value = cfg
    }

    fun connect() {
        connectGeneration++
        disconnect()
        val cfg = _config.value
        val c = DeckboardClient(cfg.host, cfg.port, cfg.accessKey)
        client = c
        observeEvents(c)
        c.connect()
    }

    fun disconnect() {
        eventJob?.cancel()
        client?.disconnect()
        client = null
        _connState.value = ConnState.Disconnected
        _boards.value = emptyList()
        _currentBoard.value = null
        _customValues.value = emptyMap()
    }

    override fun onCleared() {
        // Release the socket and the client's ping executor thread; the
        // scope itself is already being cancelled.
        disconnect()
    }

    private fun observeEvents(client: DeckboardClient) {
        eventJob = viewModelScope.launch {
            launch {
                client.state.collect { st ->
                    _connState.value = st
                    if (st is ConnState.Connected) {
                        reconnectAttempts = 0
                        // full snapshot on every (re)connect - ADR-006
                        client.requestBoards()
                    }
                    if (st is ConnState.Failed || st is ConnState.Disconnected) scheduleReconnect()
                }
            }
            launch {
                client.events.collect { ev ->
                    when (ev) {
                        is DeckEvent.Shortcuts -> {
                            val boards = runCatching {
                                json.decodeFromJsonElement<List<Board>>(ev.json)
                            }.getOrElse {
                                Log.w(TAG, "bad boards payload", it)
                                emptyList()
                            }
                            _boards.value = boards.sortedBy { it.order?.let { o -> o } ?: Int.MAX_VALUE }
                            // keep selection if still present, else first board
                            val cur = _currentBoard.value
                            _currentBoard.value = boards.firstOrNull { it.id == cur?.id } ?: boards.firstOrNull()
                        }
                        is DeckEvent.ChangeBoard -> {
                            _currentBoard.value = _boards.value.firstOrNull { it.id == ev.boardId }
                                ?: _currentBoard.value
                        }
                        DeckEvent.RefreshBoard -> client.requestBoards()
                        is DeckEvent.AppStatus -> {
                            if (ev.app == "APP_CUSTOM_VALUE") {
                                _customValues.value = _customValues.value + ev.data
                                _liveScalars.value = _liveScalars.value +
                                    ev.data.mapValues { (_, el) -> parseScalar(el) }
                                _valueHistory.value = updateHistory(_valueHistory.value, ev.data)
                            }
                        }
                        is DeckEvent.Version -> _serverVersion.value = ev.version
                        is DeckEvent.Other -> Log.d(TAG, "event ${ev.name}")
                    }
                }
            }
        }
    }

    /** Payloads are scalars ("14:33") or objects ({value, suffix}). */
    private fun parseScalar(el: JsonElement): LiveScalar {
        val obj = el as? kotlinx.serialization.json.JsonObject
        val text = (obj?.get("value") ?: el)
            .let { runCatching { it.jsonPrimitive.content }.getOrNull() }
        val suffix = obj?.get("suffix")
            ?.let { runCatching { it.jsonPrimitive.content }.getOrNull() }
        return LiveScalar(text, suffix)
    }

    private fun scheduleReconnect() {
        if (reconnectAttempts >= MAX_RECONNECT) return
        if (pendingReconnectGeneration == connectGeneration) return
        pendingReconnectGeneration = connectGeneration
        reconnectAttempts++
        val generation = connectGeneration
        viewModelScope.launch {
            delay(reconnectAttempts.coerceAtMost(6) * 2_000L)
            // free the slot: sequential failures may schedule again
            pendingReconnectGeneration = -1
            // A newer connect() cycle (user retry or a scheduled reconnect
            // that already fired) took over while we waited.
            if (generation != connectGeneration) return@launch
            val st = _connState.value
            if (st is ConnState.Failed || st is ConnState.Disconnected) {
                Log.i(TAG, "reconnect attempt $reconnectAttempts")
                connect()
            }
        }
    }

    /** Fold one `app_status_update` batch into the per-key histories. */
    private fun updateHistory(
        current: Map<String, List<Float>>,
        data: Map<String, JsonElement>,
    ): Map<String, List<Float>> {
        val out = current.toMutableMap()
        for ((key, el) in data) {
            val value = numericOf(el) ?: continue
            val series = (out[key] ?: emptyList()) + value
            out[key] = if (series.size > HISTORY_CAP) series.takeLast(HISTORY_CAP) else series
        }
        return out
    }

    private fun numericOf(el: JsonElement): Float? {
        // graph payloads are objects like {value, title, suffix}; scalars
        // are the value themselves
        val raw = when (el) {
            is kotlinx.serialization.json.JsonObject ->
                runCatching { el["value"]?.jsonPrimitive?.content }.getOrNull()
            else ->
                runCatching { el.jsonPrimitive.content }.getOrNull()
        } ?: return null
        return raw.toFloatOrNull()?.takeIf { it.isFinite() }
    }

    // -- user interactions ------------------------------------------------

    // Original behavior: only `key` buttons act on touch-down (isTapStart
    // = true); everything else executes on release. Board buttons switch
    // boards locally like the stock client (command = `{"id":N}`).
    fun holdStart(shortcut: Shortcut) {
        if (shortcut.type == "key") {
            shortcut.id?.let { client?.execShortcut(it, true) }
        }
    }

    fun holdEnd(shortcut: Shortcut) {
        if (shortcut.type == "board") {
            val target = runCatching {
                json.parseToJsonElement(shortcut.command).jsonObject["id"]
                    ?.jsonPrimitive?.content?.toLong()
            }.getOrNull()
            if (target != null) {
                _boards.value.firstOrNull { it.id == target }?.let { selectBoard(it) }
                return
            }
        }
        shortcut.id?.let { client?.execShortcut(it, false) }
    }

    fun slider(shortcut: Shortcut, value: Float) {
        shortcut.id?.let { client?.execSlider(it, value) }
    }

    fun refresh() {
        client?.requestBoards()
    }

    fun selectBoard(board: Board) {
        _currentBoard.value = board
    }

    companion object {
        private const val TAG = "DeckboardViewModel"
        private const val MAX_RECONNECT = 10
        /** The original app's PRO handshake key: full grid instead of 4x3. */
        const val PRO_ACCESS_KEY = "DCKBRD_PRO_1_3_0"
        /** The original client keeps the last 10 graph values. */
        const val HISTORY_CAP = 10
    }
}
