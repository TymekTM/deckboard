//! Protocol v2 wire models (docs/protocol-v2.md). The Rust types in
//! `crates/proto` are the source of truth; the golden fixtures in
//! `crates/proto/tests/fixtures/` are parsed by ProtoFixturesTest so this
//! file cannot drift from the wire format.
//!
//! Enum-ish wire values (kinds, interactions, shapes) are strings with
//! constants instead of Kotlin enums: kotlinx.serialization has no
//! unknown-enum fallback, and the protocol requires degrading on unknown
//! values instead of failing the parse. A [Tile] is flat on the wire
//! (placement + manifest are serde-flattened on the Rust side), which maps
//! 1:1 onto a single data class here.

package app.deckboard.mobile.proto

import androidx.compose.runtime.Immutable
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonPrimitive

/** Protocol constants. */
object V2 {
    const val PROTOCOL = 2
        /** Server ring buffer window per series channel (docs/protocol-v2.md §5). */
        const val SERIES_CAP = 120

    // message type names
    const val TYPE_HELLO = "hello"
    const val TYPE_WELCOME = "welcome"
    const val TYPE_ERROR = "error"
    const val TYPE_BOARDS_SYNC = "boards.sync"
    const val TYPE_BOARDS_DELTA = "boards.delta"
    const val TYPE_BOARD_OPEN = "board.open"
    const val TYPE_STATE_SYNC = "state.sync"
    const val TYPE_STATE_PATCH = "state.patch"
    const val TYPE_INTERACTION = "interaction"

    // widget kinds (unknown degrades to button)
    const val KIND_BUTTON = "button"
    const val KIND_TOGGLE = "toggle"
    const val KIND_SLIDER = "slider"
    const val KIND_KNOB = "knob"
    const val KIND_GRAPH = "graph"
    const val KIND_LIST = "list"

    // interactions
    const val INT_TAP = "tap"
    const val INT_PRESS_START = "press-start"
    const val INT_PRESS_END = "press-end"
    const val INT_SLIDE = "slide"

    // state shapes
    const val SHAPE_SCALAR = "scalar"
    const val SHAPE_SERIES = "series"
    const val SHAPE_TOGGLE = "toggle"
    const val SHAPE_LIST = "list"
}

@Serializable
data class Frame(
    val v: Int,
    val id: String? = null,
    val ack: String? = null,
    @SerialName("type") val type: String,
    val payload: JsonElement? = null,
)

@Serializable
data class Hello(
    val client: String,
    val version: String,
    val name: String? = null,
    val capabilities: List<String> = emptyList(),
)

@Serializable
data class Device(
    val id: String,
    val name: String,
)

@Serializable
data class ChannelInfo(
    val shape: String,
    val cap: Int? = null,
)

@Serializable
data class Welcome(
    val protocol: Int,
    @SerialName("desktop_version") val desktopVersion: String,
    @SerialName("min_client") val minClient: String,
    val generation: Long,
    val device: Device,
    /** Issued only in the welcome that completes a pairing. */
    val token: String? = null,
    val channels: Map<String, ChannelInfo> = emptyMap(),
)

@Serializable
data class ErrorPayload(
    val code: String,
    val message: String? = null,
)

@Immutable
@Serializable
data class Board(
    val id: Long,
    val name: String = "",
    val width: Int = 4,
    val height: Int = 3,
    val order: Int = 0,
    val background: Background? = null,
    val tiles: List<Tile> = emptyList(),
)

/** Board background: `{"kind":"color","color":..}` or
 *  `{"kind":"asset","hash":..}` - one flat class, the kind picks the
 *  meaningful field. */
@Immutable
@Serializable
data class Background(
    val kind: String,
    val color: String? = null,
    val hash: String? = null,
) {
    companion object {
        fun color(value: String) = Background(kind = "color", color = value)
    }
}

internal fun JsonElement.contentOrNull(): String? =
    (this as? JsonPrimitive)?.let { runCatching { it.content }.getOrNull() }

/** One tile: placement + widget manifest flattened into one object. */
@Immutable
@Serializable
data class Tile(
    val id: Long,
    val x: Int = 0,
    val y: Int = 0,
    val w: Int = 1,
    val h: Int = 1,
    val kind: String = V2.KIND_BUTTON,
    val params: JsonElement? = null,
    val state: StateRef? = null,
    val interactions: List<String> = emptyList(),
    val style: Style? = null,
    @SerialName("web_package") val webPackage: String? = null,
    @SerialName("asset_hash") val assetHash: String? = null,
) {
    fun interacts(kind: String): Boolean = interactions.contains(kind)

    /** `params.widget` - implicit template hints (e.g. the clock). */
    fun widgetHint(): String? =
        (params as? JsonObject)?.get("widget")?.contentOrNull()

    fun param(name: String): String? =
        (params as? JsonObject)?.get(name)?.contentOrNull()
}

@Immutable
@Serializable
data class StateRef(
    val channel: String,
    val shape: String = V2.SHAPE_SCALAR,
)

@Immutable
@Serializable
data class Style(
    val color: String? = null,
    val color2: String? = null,
    val icon: String? = null,
    val icon2: String? = null,
    @SerialName("icon_family") val iconFamily: String? = null,
    val title: String? = null,
    val shape: String? = null,
)

@Serializable
data class BoardsSync(
    val generation: Long,
    val boards: List<Board> = emptyList(),
)

/** One committed board change; ops arrive tagged with a `"op"` field. */
sealed class BoardOp {
    data class BoardSet(val board: Board) : BoardOp()
    data class BoardRemove(val boardId: Long) : BoardOp()
    data class TileSet(val boardId: Long, val tile: Tile) : BoardOp()
    data class TileRemove(val boardId: Long, val tileId: Long) : BoardOp()
    data class TileClear(val boardId: Long) : BoardOp()

    companion object {
        /** Unknown ops return null (ignored per protocol evolution rules). */
        fun from(el: JsonElement, json: Json): BoardOp? {
            val obj = el as? JsonObject ?: return null
            val op = obj["op"]?.contentOrNull() ?: return null
            val boardId = obj["board"]?.jsonPrimitive?.content?.toLongOrNull()
            return when (op) {
                "board-set" -> obj["board"]?.let {
                    json.decodeFromString(Board.serializer(), it.toString())
                }?.let { BoardSet(it) }
                "board-remove" -> boardId?.let { BoardRemove(it) }
                "tile-set" -> boardId?.let { b ->
                    obj["tile"]?.let { json.decodeFromString(Tile.serializer(), it.toString()) }
                        ?.let { TileSet(b, it) }
                }
                "tile-remove" -> boardId?.let { b ->
                    obj["tile"]?.jsonPrimitive?.content?.toLongOrNull()?.let { TileRemove(b, it) }
                }
                "tile-clear" -> boardId?.let { TileClear(it) }
                else -> null
            }
        }
    }
}

@Serializable
data class BoardsDelta(
    val generation: Long,
    val ops: List<JsonElement> = emptyList(),
)

@Serializable
data class BoardOpen(
    val board: Long,
)

@Serializable
data class StateSync(
    val values: Map<String, JsonElement> = emptyMap(),
    val series: Map<String, List<Double>> = emptyMap(),
)

@Serializable
data class ChannelValue(
    val channel: String,
    val value: JsonElement,
)

@Serializable
data class StatePatch(
    val changes: List<ChannelValue> = emptyList(),
)

@Serializable
data class InteractionArgs(
    val value: Double? = null,
    val delta: Double? = null,
    val dx: Double? = null,
    val dy: Double? = null,
)

@Serializable
data class InteractionPayload(
    val board: Long,
    val tile: Long,
    val interaction: String,
    val args: InteractionArgs = InteractionArgs(),
)
