//! Stable value wrappers for tile inputs (docs: Compose treats raw
//! collections as unstable, so an unchanged list would still recompose
//! every tile on each state patch). Equality is by content.

package app.pulpit.mobile.ui

import androidx.compose.runtime.Immutable

@Immutable
data class SeriesWindow(val points: List<Double>)

@Immutable
data class TileItems(val values: List<String>)
