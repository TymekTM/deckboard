//! Wire models mirroring the legacy payload contract (see
//! crates/legacy/src/mapping.rs - field names are contractual; the stock
//! Android client renders exactly these).

package app.pulpit.mobile.proto

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement

@Serializable
data class Board(
    val id: Long? = null,
    val name: String? = null,
    val background: String? = null,
    @SerialName("width") val width: Int = 4,
    @SerialName("height") val height: Int = 3,
    val staggered: Boolean = false,
    val order: Int? = null,
    val shortcuts: List<Shortcut> = emptyList(),
)

@Serializable
data class Shortcut(
    val id: Long? = null,
    @SerialName("board_id") val boardId: Long? = null,
    val type: String = "",
    val command: String = "",
    val mode: String = "button",
    val extra: String = "",
    val app: String? = null,
    @SerialName("toggle_key") val toggleKey: String? = null,
    val x: Int = 0,
    val y: Int = 0,
    val w: Int = 1,
    val h: Int = 1,
    // nullable in the payload: filler cells carry position: null
    val position: Int? = null,
    val color: String = "#34495E",
    val color2: String = "",
    val img: String? = null,
    @SerialName("icon_color") val iconColor: String? = null,
    @SerialName("icon_color2") val iconColor2: String? = null,
    val title: String? = null,
    @SerialName("title_position") val titlePosition: Int = 0,
    @SerialName("title_position2") val titlePosition2: Int = 0,
    @SerialName("title_color") val titleColor: String = "#ffffff",
    @SerialName("title_color2") val titleColor2: String = "#ffffff",
    @SerialName("title_box_color") val titleBoxColor: String? = null,
    @SerialName("title_box_color2") val titleBoxColor2: String? = null,
    @SerialName("border_color") val borderColor: String? = null,
    @SerialName("border_color2") val borderColor2: String? = null,
    val shape: Int = 0,
    val shape2: Int = 0,
    val unicode: String = "",
    val unicode2: String = "",
    val prefix: String = "fas",
    val options: JsonElement? = null,
)

/// `app_status_update {app, data}` - live state pushed to clients.
@Serializable
data class AppStatus(
    val app: String = "",
    val data: Map<String, JsonElement> = emptyMap(),
)

@Serializable
data class BoardChange(
    @SerialName("boardId") val boardId: Long = 0,
)

@Serializable
data class VersionInfo(
    val version: String = "",
)
