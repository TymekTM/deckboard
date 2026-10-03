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

@Immutable
data class StatusData(
    val rows: List<StatusRow>,
    val compact: List<StatusCompact> = emptyList(),
    val summary: String = "",
    /** Row identifier style: "name" (default) or "logo" - never both. */
    val rowStyle: String = "name",
)
