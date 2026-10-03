//! Protocol v2 WebSocket client (docs/protocol-v2.md): one plain WS to
//! `/v2/ws`, `hello` on open, typed events out, interaction helpers in.
//! Keepalive is server-driven (WS pings); this side answers pongs
//! (OkHttp does automatically) and closes when the server is silent for
//! longer than the watchdog window - reconnection is the caller's job.

package app.pulpit.mobile.net

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
import app.pulpit.mobile.proto.decodeDeltaOps
import app.pulpit.mobile.state.Plog
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
    /** [retryable] = false: the desktop refused this device or app for
     *  good (unknown token, bad pairing code, outdated client). The same
     *  credentials can never succeed, so nothing reconnects on its own. */
    data class Failed(val reason: String, val retryable: Boolean = true) : ConnState()
    /** The server sent `server.shutdown`: the exit is deliberate, and
     *  reconnecting would be pointless until it comes back. */
    data object ServerDown : ConnState()
}

/** Terminal states outlive the close/failure callbacks that follow them. */
fun ConnState.isTerminal(): Boolean =
    this is ConnState.ServerDown || (this is ConnState.Failed && !retryable)

/** User-facing text for a fatal refusal (an error-frame code, or
 *  "unauthorized" for the upgrade's HTTP 401). */
fun fatalReason(code: String): String = when (code) {
    "pair-invalid" -> "invalid pairing code - generate a new one on the desktop"
    "pair-expired" -> "pairing code expired - generate a new one on the desktop"
    "unauthorized" -> "device revoked on the desktop - pair again"
    "outdated-client" -> "this app is too old for the desktop - update it"
    else -> "the desktop refused the connection ($code)"
}

/** State for a socket failure. The `/v2/ws` upgrade answers 401 for an
 *  unknown or revoked token (crates/v2/src/service.rs), and retrying
 *  the same token cannot work. Everything else is a transient drop. */
internal fun failureState(httpCode: Int?, message: String?): ConnState.Failed =
    if (httpCode == 401) {
        ConnState.Failed(fatalReason("unauthorized"), retryable = false)
    } else {
        ConnState.Failed(message ?: "connection failed")
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
        // MOB-01: a malformed host/port (saved by an older build, or a
        // 5-digit port that passed the digit filter) must surface as a
        // Failed state, never as OkHttp's IllegalArgumentException
        // through the caller - a paired device would crash-loop at
        // every launch otherwise. The same bad address can never
        // connect, so the failure is terminal.
        val invalid = addressError(host, port)
        if (invalid != null) {
            Plog.w(TAG, "invalid address \"$host:$port\" - not connecting")
            _state.value = ConnState.Failed(invalid, retryable = false)
            return
        }
        val url = "ws://$host:$port/v2/ws?$auth"
        Plog.i(TAG, "connecting to ws://$host:$port/v2/ws")
        _state.value = ConnState.Connecting(host, port)
        webSocket = runCatching {
            http.newWebSocket(Request.Builder().url(url).build(), listener)
        }.getOrElse {
            Plog.w(TAG, "cannot build socket URL for \"$host:$port\": ${it.message}")
            _state.value = ConnState.Failed("invalid address: \"$host:$port\"", retryable = false)
            null
        }
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

    /** M5 custom gestures ride the ordinary interaction frame; the
     *  ViewModel gates on the tile's declared interactions. */
    fun gesture(boardId: Long, tileId: Long, name: String) {
        sendInteraction(boardId, tileId, name, null)
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
        Plog.i(TAG, "server is shutting down - standing down")
        _state.value = ConnState.ServerDown
        webSocket?.close(1000, "server shutdown acknowledged")
    }

    private fun sendFrame(frame: Frame) {
        val text = json.encodeToString(Frame.serializer(), frame)
        if (webSocket?.send(text) != true) {
            Plog.w(TAG, "frame dropped, socket closed")
        }
    }

    private val listener = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            Plog.i(TAG, "socket open, sending hello")
            sendFrame(
                Frame(
                    v = V2.PROTOCOL,
                    id = nextRequestId(),
                    type = V2.TYPE_HELLO,
                    payload = json.encodeToJsonElement(
                        Hello.serializer(),
                        Hello(
                            client = CLIENT,
                            version = VERSION,
                            name = deviceName.ifBlank { null },
                            capabilities = CLIENT_CAPABILITIES,
                        ),
                    ),
                ),
            )
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            runCatching { handleFrame(text) }
                .onFailure {
                    // MOB-12: a frame can be up to 1 MiB - dump only its
                    // head into the log. A malformed welcome would
                    // otherwise land the fresh pairing token it carries
                    // in logcat and the field log (ADR-008: never token
                    // material).
                    Plog.w(
                        TAG,
                        "bad frame (${it.javaClass.simpleName}: ${it.message}): ${text.take(BAD_FRAME_LOG_CHARS)}",
                    )
                }
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            Plog.i(TAG, "closed: $reason")
            setStateUnlessTerminal(ConnState.Disconnected)
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            Plog.w(TAG, "failure: ${t.message} (http ${response?.code})")
            setStateUnlessTerminal(failureState(response?.code, t.message))
        }
    }

    /** Terminal states (the goodbye, or a refusal) survive the close or
     *  failure that follows them: the socket going away is the expected
     *  aftermath, not a retryable drop. */
    private fun setStateUnlessTerminal(state: ConnState) {
        if (!_state.value.isTerminal()) {
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
                val ops = decodeDeltaOps(delta.ops, json)
                if (ops == null) {
                    // One op this client cannot parse: the local board
                    // snapshot is no longer trustworthy. Close the socket
                    // so the reconnect path fetches a fresh boards.sync -
                    // staying would drift silently from the desktop.
                    Plog.w(TAG, "rejected boards.delta op - reconnecting for a fresh snapshot")
                    webSocket?.close(1000, "delta op rejected")
                    return
                }
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
                Plog.w(TAG, "server error: ${error.code} ${error.message.orEmpty()}")
                val fatal = FATAL_CODES.contains(error.code)
                if (fatal) {
                    // Terminal before the event and the close, so the
                    // onClosed that follows cannot downgrade it.
                    setStateUnlessTerminal(ConnState.Failed(fatalReason(error.code), retryable = false))
                }
                _events.trySend(V2Event.ServerError(error.code, error.message))
                if (fatal) {
                    webSocket?.close(1000, error.code)
                }
            }
            else -> {
                val ack = frame.ack
                if (ack != null) {
                    _events.trySend(V2Event.Acked(ack))
                } else {
                    Plog.i(TAG, "ignored frame type ${frame.type}")
                }
            }
        }
    }

    companion object {
        private const val TAG = "V2Client"

        /** How much of an undecodable frame reaches the log (MOB-12):
         *  frames can carry up to 1 MiB, and a malformed welcome would
         *  otherwise print the pairing token it contains. */
        private const val BAD_FRAME_LOG_CHARS = 256

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

        /** M5 capability negotiation: what this client renders/accepts,
         *  declared in every hello. The list mirrors the UI's actual
         *  surface (widget kinds, live-state features, gestures); new
         *  capabilities join when the client really implements them. */
        val CLIENT_CAPABILITIES = listOf(
            "kinds:button",
            "kinds:toggle",
            "kinds:slider",
            "kinds:knob",
            "kinds:graph",
            "kinds:list",
            "series",
            "state.patch",
            "assets",
            "assets2",
            "gestures:long-press",
            "gestures:double-tap",
            "gestures:swipe-left",
            "gestures:swipe-right",
        )
        /** Client version reported in hello. Derived from versionName,
         *  which build.gradle.kts reads out of the workspace Cargo.toml -
         *  the single version source (this used to be a drifting
         *  literal). */
        val VERSION = app.pulpit.mobile.BuildConfig.VERSION_NAME
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
