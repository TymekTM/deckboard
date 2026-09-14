package app.deckboard.mobile.proto

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement

/**
 * Protocol v2 wire models (docs/protocol-v2.md). The Rust types in
 * `crates/proto` are the source of truth; the golden fixtures in
 * `crates/proto/tests/fixtures/` are parsed by [ProtoFixturesTest] so this
 * file cannot drift from the wire format. Full Tile typing lands with the
 * M4 client work - tiles stay [JsonElement] until then.
 */
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

/** Shape names kept as strings: kotlinx.serialization has no unknown-enum
 * fallback, so unknown future shapes must not break parsing (they render
 * as scalar client-side, mirroring Rust's `#[serde(other)]`). */
object Shapes {
    const val SCALAR = "scalar"
    const val SERIES = "series"
    const val TOGGLE = "toggle"
    const val LIST = "list"
}

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
    val channels: Map<String, ChannelInfo> = emptyMap(),
)

@Serializable
data class ErrorPayload(
    val code: String,
    val message: String? = null,
)

@Serializable
data class BoardsSync(
    val generation: Long,
    val boards: List<JsonElement> = emptyList(),
)

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
