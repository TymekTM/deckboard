//! Protocol v2 WebSocket client (docs/protocol-v2.md): one plain WS to
//! `/v2/ws`, `hello` on open, typed events out, interaction helpers in.
//! Keepalive is server-driven (WS pings); this side answers pongs
//! (OkHttp does automatically) and closes when the server is silent for
//! longer than the watchdog window - reconnection is the caller's job.

package app.pulpit.mobile.net

import android.util.Log
import app.pulpit.mobile.proto.BoardsDelta
import app.pulpit.mobile.proto.BoardsSync
import app.pulpit.mobile.proto.BoardOp
import app.pulpit.mobile.proto.BoardOpen
import app.pulpit.mobile.proto.ChannelValue
import app.pulpit.mobile.proto.ErrorPayload
import app.pulpit.mobile.proto.Frame
import app.pulpit.mobile.proto.Hello
import app.pulpit.mobile.proto.InteractionArgs
import app.pulpit.mobile.proto.InteractionPayload
import app.pulpit.mobile.proto.StateSync
import app.pulpit.mobile.proto.V2
import app.pulpit.mobile.proto.Welcome
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.encodeToJsonElement
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
    /** The server sent `server.shutdown`: the exit is deliberate, and
     *  reconnecting would be pointless until it comes back. */
    data object ServerDown : ConnState()
}

/** One decoded server frame, ready for the ViewModel. */
sealed class V2Event {
    data class WelcomeReady(
        val welcome: Welcome,
        /** Non-null only right after pairing: store it. */
        val issuedToken: String?,
    ) : V2Event()
    data class Boards(val generation: Long, val boards: List<app.pulpit.mobile.proto.Board>) : V2Event()
    data class Delta(val generation: Long, val ops: List<BoardOp>) : V2Event()
    data class SwitchBoard(val boardId: Long) : V2Event()
    data class State(
        val values: Map<String, JsonElement>,
        val series: Map<String, List<Double>>,
    ) : V2Event()
    data class Patch(val changes: List<ChannelValue>) : V2Event()
    data class ServerError(val code: String, val message: String?) : V2Event()
    /** An interaction (or hello) was accepted. */
    data class Acked(val requestId: String) : V2Event()
}

class V2Client(
    private val host: String,
    private val port: Int,
    /** Paired-device token; null when pairing with [pairCode]. */
    private val token: String?,
    private val pairCode: String?,
    private val deviceName: String,
) {
    private val json = Json { ignoreUnknownKeys = true; isLenient = true }

    private val _state = MutableStateFlow<ConnState>(ConnState.Connecting(host, port))
    val state: StateFlow<ConnState> = _state

    private val _events = kotlinx.coroutines.channels.Channel<V2Event>(
        kotlinx.coroutines.channels.Channel.UNLIMITED,
    )
    val events: kotlinx.coroutines.channels.ReceiveChannel<V2Event> = _events

    private var webSocket: WebSocket? = null
    private var requestCounter = 0

    private val http: OkHttpClient = Companion.http

    fun connect() {
        val auth = when {
            !token.isNullOrBlank() -> "token=$token"
            !pairCode.isNullOrBlank() -> "pair=$pairCode"
            else -> {
                _state.value = ConnState.Failed("no token or pairing code")
                return
            }
        }
        val url = "ws://$host:$port/v2/ws?$auth"
        Log.i(TAG, "connecting to ws://$host:$port/v2/ws")
        _state.value = ConnState.Connecting(host, port)
        webSocket = http.newWebSocket(Request.Builder().url(url).build(), listener)
    }

    fun disconnect() {
        _events.close()
        webSocket?.close(1000, "bye")
        webSocket = null
        _state.value = ConnState.Disconnected
    }

    // -- interactions -----------------------------------------------------

    fun tap(boardId: Long, tileId: Long) {
        sendInteraction(boardId, tileId, V2.INT_TAP, null)
    }

    fun pressStart(boardId: Long, tileId: Long) {
        sendInteraction(boardId, tileId, V2.INT_PRESS_START, null)
    }

    fun pressEnd(boardId: Long, tileId: Long) {
        sendInteraction(boardId, tileId, V2.INT_PRESS_END, null)
    }

    fun slide(boardId: Long, tileId: Long, value: Float) {
        sendInteraction(boardId, tileId, V2.INT_SLIDE, InteractionArgs(value = value.toDouble()))
    }

    private fun sendInteraction(boardId: Long, tileId: Long, interaction: String, args: InteractionArgs?) {
        val payload = InteractionPayload(
            board = boardId,
            tile = tileId,
            interaction = interaction,
            args = args ?: InteractionArgs(),
        )
        sendFrame(
            Frame(
                v = V2.PROTOCOL,
                id = nextRequestId(),
                type = V2.TYPE_INTERACTION,
                payload = json.encodeToJsonElement(InteractionPayload.serializer(), payload),
            ),
        )
    }

    // -- internals ---------------------------------------------------------

    private fun nextRequestId(): String = "c${requestCounter++}"

    /** The server's goodbye (docs/protocol-v2.md §9): the exit is
     *  deliberate. Surface the terminal state, then close politely so the
     *  server's teardown sees an acked peer. */
    private fun onServerShutdown() {
        Log.i(TAG, "server is shutting down - standing down")
        _state.value = ConnState.ServerDown
        webSocket?.close(1000, "server shutdown acknowledged")
    }

    private fun sendFrame(frame: Frame) {
        val text = json.encodeToString(Frame.serializer(), frame)
        if (webSocket?.send(text) != true) {
            Log.w(TAG, "frame dropped, socket closed")
        }
    }

    private val listener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            Log.i(TAG, "socket open, sending hello")
            sendFrame(
                Frame(
                    v = V2.PROTOCOL,
                    id = nextRequestId(),
                    type = V2.TYPE_HELLO,
                    payload = json.encodeToJsonElement(
                        Hello.serializer(),
                        Hello(client = CLIENT, version = VERSION, name = deviceName.ifBlank { null }),
                    ),
                ),
            )
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            runCatching { handleFrame(text) }
                .onFailure { Log.w(TAG, "bad frame: $text", it) }
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            Log.i(TAG, "closed: $reason")
            setStateUnlessServerDown(ConnState.Disconnected)
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            Log.w(TAG, "failure: ${t.message}")
            setStateUnlessServerDown(ConnState.Failed(t.message ?: "connection failed"))
        }
    }

    /** The terminal `ServerDown` state survives the close or failure that
     *  follows it: the socket going away is the expected aftermath of the
     *  goodbye, not a retryable drop. */
    private fun setStateUnlessServerDown(state: ConnState) {
        if (_state.value !is ConnState.ServerDown) {
            _state.value = state
        }
    }

    private fun handleFrame(text: String) {
        val frame = json.decodeFromString(Frame.serializer(), text)
        // The shutdown notice must land even if a future server sends it
        // without a payload - it carries nothing worth parsing anyway.
        if (frame.type == V2.TYPE_SERVER_SHUTDOWN) {
            onServerShutdown()
            return
        }
        // Dispatch on the frame type first: acks ECHO the request's type
        // (the welcome that confirms the hello and interaction acks carry
        // `ack` too), so matching on `ack` alone would swallow them.
        val payload = frame.payload ?: return
        when (frame.type) {
            V2.TYPE_WELCOME -> {
                val welcome = json.decodeFromJsonElement(Welcome.serializer(), payload)
                _state.value = ConnState.Connected(host, port)
                _events.trySend(V2Event.WelcomeReady(welcome, welcome.token))
            }
            V2.TYPE_BOARDS_SYNC -> {
                val sync = json.decodeFromJsonElement(BoardsSync.serializer(), payload)
                _events.trySend(V2Event.Boards(sync.generation, sync.boards))
            }
            V2.TYPE_BOARDS_DELTA -> {
                val delta = json.decodeFromJsonElement(BoardsDelta.serializer(), payload)
                val ops = delta.ops.mapNotNull { BoardOp.from(it, json) }
                _events.trySend(V2Event.Delta(delta.generation, ops))
            }
            V2.TYPE_BOARD_OPEN -> {
                val open = json.decodeFromJsonElement(BoardOpen.serializer(), payload)
                _events.trySend(V2Event.SwitchBoard(open.board))
            }
            V2.TYPE_STATE_SYNC -> {
                val sync = json.decodeFromJsonElement(StateSync.serializer(), payload)
                _events.trySend(V2Event.State(sync.values, sync.series))
            }
            V2.TYPE_STATE_PATCH -> {
                val patch = json.decodeFromJsonElement(
                    app.pulpit.mobile.proto.StatePatch.serializer(),
                    payload,
                )
                _events.trySend(V2Event.Patch(patch.changes))
            }
            V2.TYPE_ERROR -> {
                val error = json.decodeFromJsonElement(ErrorPayload.serializer(), payload)
                Log.w(TAG, "server error: ${error.code} ${error.message.orEmpty()}")
                _events.trySend(V2Event.ServerError(error.code, error.message))
                if (FATAL_CODES.contains(error.code)) {
                    webSocket?.close(1000, error.code)
                }
            }
            else -> {
                val ack = frame.ack
                if (ack != null) {
                    _events.trySend(V2Event.Acked(ack))
                } else {
                    Log.d(TAG, "ignored frame type ${frame.type}")
                }
            }
        }
    }

    companion object {
        private const val TAG = "V2Client"

        /** One client per process. OkHttp keeps its own thread pools and
         *  connection pool; building an instance per reconnect would leak
         *  them until GC on a device that reconnects for a living. */
        val http: OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(6, TimeUnit.SECONDS)
            .readTimeout(0, TimeUnit.MILLISECONDS)
            // Liveness: OkHttp pings every KEEPALIVE_SECONDS and fails the
            // socket when a pong is missing (keepalive is protocol-level,
            // docs/protocol-v2.md §7 - no app frames).
            .pingInterval(KEEPALIVE_SECONDS, TimeUnit.SECONDS)
            .build()
        const val CLIENT = "pulpit-mobile"
        const val VERSION = "0.2.0"
        /** OkHttp ping interval; a missing pong fails the socket. */
        const val KEEPALIVE_SECONDS = 30L
        /** Fatal errors close the socket; no point retrying with the same auth. */
        val FATAL_CODES = setOf("unauthorized", "pair-invalid", "pair-expired", "outdated-client", "too-large")
    }
}

/** Turn a pushed channel value into display text: scalars render as-is,
 *  `{value, suffix}` objects render as `value + suffix` (the shape
 *  extensions have always pushed). */
fun displayText(value: JsonElement?): String? {
    val el = value ?: return null
    val obj = el as? JsonObject
    val valueEl = obj?.get("value") ?: el
    val text = (valueEl as? JsonPrimitive)?.let { runCatching { it.content }.getOrNull() } ?: return null
    val suffix = obj?.get("suffix")?.let { (it as? JsonPrimitive)?.content }
    return text + suffix.orEmpty()
}

/** Whether a pushed toggle/state value means "active". */
fun isActiveValue(value: JsonElement?): Boolean {
    val primitive = value as? JsonPrimitive ?: return false
    if (primitive.isString) {
        val text = runCatching { primitive.content }.getOrNull() ?: return false
        return text == "ON" || text == "1"
    }
    return primitive.booleanOrNull ?: false
}

/** Graph series value for display; from a pushed object `{value}` or a raw number. */
fun numericValue(value: JsonElement?): Double? {
    val obj = value as? JsonObject
    val el = obj?.get("value") ?: value ?: return null
    return (el as? JsonPrimitive)?.doubleOrNull
}
