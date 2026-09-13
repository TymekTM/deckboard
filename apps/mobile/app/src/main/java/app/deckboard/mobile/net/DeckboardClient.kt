//! socket.io v2 / Engine.IO v3 client over a plain WebSocket transport,
//! wire-compatible with the original desktop server and deckboard-server.
//!
//! Frames: engine packets ("0" open, "2"/"3" ping/pong, "4" message) with
//! socket.io events inside ("40" connect, `42["event",args]`).

package app.deckboard.mobile.net

import android.util.Log
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.util.concurrent.TimeUnit

sealed class ConnState {
    data object Disconnected : ConnState()
    data class Connecting(val host: String, val port: Int) : ConnState()
    data class Connected(val host: String, val port: Int) : ConnState()
    data class Failed(val reason: String) : ConnState()
}

/** One decoded server event. */
sealed class DeckEvent {
    data class Shortcuts(val json: JsonElement) : DeckEvent()
    data class ChangeBoard(val boardId: Long) : DeckEvent()
    data object RefreshBoard : DeckEvent()
    data class AppStatus(val app: String, val data: Map<String, JsonElement>) : DeckEvent()
    data class Version(val version: String) : DeckEvent()
    data class Other(val name: String, val args: JsonArray) : DeckEvent()
}

class DeckboardClient(
    private val host: String,
    private val port: Int,
    private val accessKey: String,
) {
    private val json = Json { ignoreUnknownKeys = true; isLenient = true }

    private val _state = MutableStateFlow<ConnState>(ConnState.Connecting(host, port))
    val state: StateFlow<ConnState> = _state

    private val _events = MutableSharedFlow<DeckEvent>(
        replay = 0, extraBufferCapacity = 64, onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )
    val events: SharedFlow<DeckEvent> = _events

    private var webSocket: WebSocket? = null
    private val pingIntervalMs = 20_000L
    @Volatile private var lastPong = 0L
    private val pingExecutor = java.util.concurrent.Executors.newSingleThreadScheduledExecutor()

    private val http = OkHttpClient.Builder()
        .connectTimeout(6, TimeUnit.SECONDS)
        .pingInterval(0, TimeUnit.MILLISECONDS) // EIO pings are app-level
        .readTimeout(0, TimeUnit.MILLISECONDS)
        .build()

    fun connect() {
        val url = "ws://$host:$port/socket.io/?EIO=3&transport=websocket&access_key=$accessKey"
        Log.i(TAG, "connecting to $url")
        val request = Request.Builder().url(url).build()
        webSocket = http.newWebSocket(request, listener)
    }

    fun disconnect() {
        pingExecutor.shutdownNow()
        webSocket?.close(1000, "bye")
        webSocket = null
        _state.value = ConnState.Disconnected
    }

    private val listener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            Log.i(TAG, "socket open")
            lastPong = System.currentTimeMillis()
            // EIO websocket transport: server sends open packet first
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            handleEngineFrame(text)
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            Log.i(TAG, "closed: $reason")
            _state.value = ConnState.Disconnected
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            Log.w(TAG, "failure: ${t.message}")
            _state.value = ConnState.Failed(t.message ?: "connection failed")
        }
    }

    private fun handleEngineFrame(text: String) {
        when (text.firstOrNull()) {
            '0' -> {
                // open packet: finish socket.io namespace connect
                webSocket?.send("40")
                _state.value = ConnState.Connected(host, port)
                startPingLoop()
                send("get_version")
                announceCapabilities()
            }
            '2' -> webSocket?.send("3") // engine ping -> pong
            '3' -> lastPong = System.currentTimeMillis()
            '4' -> handleSioFrame(text.drop(1))
        }
    }

    private fun handleSioFrame(sio: String) {
        when (sio.firstOrNull()) {
            '2' -> {
                // `42["event",...]`
                val payload = sio.drop(1)
                runCatching {
                    val arr = json.decodeFromString(JsonArray.serializer(), payload)
                    val name = arr.first().jsonPrimitive.content
                    val ev = when (name) {
                        "get_shortcuts" -> DeckEvent.Shortcuts(arr.getOrNull(1) ?: JsonArray(emptyList()))
                        "change_board" -> DeckEvent.ChangeBoard(
                            json.decodeFromJsonElement<app.deckboard.mobile.proto.BoardChange>(arr.getOrNull(1) ?: JsonObject(emptyMap())).boardId,
                        )
                        "refresh_board" -> DeckEvent.RefreshBoard
                        "app_status_update" -> {
                            val status = json.decodeFromJsonElement<app.deckboard.mobile.proto.AppStatus>(
                                arr.getOrNull(1) ?: JsonObject(emptyMap()),
                            )
                            DeckEvent.AppStatus(status.app, status.data)
                        }
                        "get_version" -> DeckEvent.Version(
                            json.decodeFromJsonElement<app.deckboard.mobile.proto.VersionInfo>(
                                arr.getOrNull(1) ?: JsonObject(emptyMap()),
                            ).version,
                        )
                        else -> DeckEvent.Other(name, arr)
                    }
                    _events.tryEmit(ev)
                }.onFailure { Log.w(TAG, "bad event frame: $payload") }
            }
            // '0' namespace ack, '3' ack: nothing to do for this client
        }
    }

    private fun startPingLoop() {
        pingExecutor.scheduleAtFixedRate({
            try {
                if (System.currentTimeMillis() - lastPong > (pingIntervalMs + 15_000)) {
                    Log.w(TAG, "server silent, reconnecting")
                    webSocket?.close(1000, "timeout")
                    return@scheduleAtFixedRate
                }
                webSocket?.send("2")
            } catch (_: Exception) {
            }
        }, pingIntervalMs, pingIntervalMs, TimeUnit.MILLISECONDS)
    }

    /** Send a socket.io event with string-serialized args. */
    fun send(event: String, args: List<JsonElement> = emptyList()) {
        val payload = buildString {
            append("42[\"").append(event).append('"')
            for (a in args) {
                append(',')
                append(json.encodeToString(JsonElement.serializer(), a))
            }
            append(']')
        }
        webSocket?.send(payload)
    }

    fun execShortcut(id: Long, isTapStart: Boolean) {
        send(
            "exec_shortcut",
            listOf(json.parseToJsonElement("""{"id":$id,"isTapStart":$isTapStart}""")),
        )
    }

    fun execSlider(id: Long, value: Float) {
        val v = value.toString()
        send(
            "exec_slider",
            listOf(json.parseToJsonElement("""{"id":$id,"value":$v}""")),
        )
    }

    fun requestBoards() {
        send("get_shortcuts")
    }

    /** M5 widget kit: declare which templates this client renders so the
     *  server can tailor payloads (unknown servers just log it). */
    fun announceCapabilities() {
        send(
            "client_capabilities",
            listOf(
                json.parseToJsonElement(
                    """{"client":"deckboard-mobile","version":"0.1.0","capabilities":["button","toggle","slider","knob","graph","list","custom-value"]}""",
                ),
            ),
        )
    }

    companion object {
        private const val TAG = "DeckboardClient"
    }
}
