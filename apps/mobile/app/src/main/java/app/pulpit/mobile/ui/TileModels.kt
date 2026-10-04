//! Stable value wrappers for tile inputs (docs: Compose treats raw
//! collections as unstable, so an unchanged list would still recompose
//! every tile on each state patch). Equality is by content.

package app.pulpit.mobile.ui

import androidx.compose.runtime.Immutable

@Immutable
data class SeriesWindow(val points: List<Double>)

@Immutable
data class TileItems(val values: List<String>)

/** One row of an ai-dev status push (rows carry label/value/state, plus a
 *  percent for plan-limit bars and a provider key for the brand glyph). */
@Immutable
data class StatusRow(
    val label: String = "",
    val value: String = "",
    val state: String = "off",
    val percent: Double? = null,
    val provider: String = "",
    val isHeader: Boolean = false,
)

/** Compact per-provider counts (`[{provider, count, state}]`) the producer
 *  ships alongside the rows for tiles too small for the detail. */
@Immutable
data class StatusCompact(
    val provider: String,
    val count: Int,
    val state: String = "working",
)

/** Optional playback progress on a status push (`spotify-now-playing`,
 *  but parsed for any producer that ships it): the position at push time
 *  plus whether the track keeps playing. Clients extrapolate locally -
 *  no server timestamp rides the wire because clocks differ. */
@Immutable
data class StatusProgress(
    val positionMs: Long,
    val durationMs: Long,
    val playing: Boolean,
)

@Immutable
data class StatusData(
    val rows: List<StatusRow>,
    val compact: List<StatusCompact> = emptyList(),
    val summary: String = "",
    /** Row identifier style: "name" (default) or "logo" - never both. */
    val rowStyle: String = "name",
    /** Album art as an asset hash (the v2 AssetStore entry); null when
     *  nothing plays, art failed, or the producer ships none. Resolved
     *  through the regular tile-asset fetch + bitmap LRU. */
    val image: String? = null,
    /** Playback progress; null when nothing plays. */
    val progress: StatusProgress? = null,
)
