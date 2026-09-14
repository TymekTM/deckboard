//! Board screen: staggered grid laid out exactly like the stock client -
//! tiles are placed at (x, y) with size (w, h) on a board.width x
//! board.height grid. No top bar: a small floating chip (board name) opens
//! the board menu; live values come from merged APP_CUSTOM_VALUE data.

package app.deckboard.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonPrimitive
import app.deckboard.mobile.proto.Board
import app.deckboard.mobile.state.DeckboardViewModel

private fun hex(color: String?, fallback: Color): Color =
    color?.let { runCatching { Color(android.graphics.Color.parseColor(it.trim())) }.getOrNull() }
        ?: fallback

@Composable
fun BoardScreen(vm: DeckboardViewModel) {
    val board by vm.currentBoard.collectAsState()

    Box(Modifier.fillMaxSize().background(DeckColors.background)) {
        board?.let { b ->
            BoardGrid(vm, b, Modifier.fillMaxSize())
        }
        Box(Modifier.align(Alignment.BottomCenter).padding(bottom = 6.dp)) {
            BoardChip(vm)
        }
    }
}

/** Small translucent board switcher in the corner - replaces the top bar. */
@Composable
private fun BoardChip(vm: DeckboardViewModel) {
    val boards by vm.boards.collectAsState()
    val board by vm.currentBoard.collectAsState()
    var menuOpen by remember { mutableStateOf(false) }

    Box(Modifier.padding(10.dp)) {
        Text(
            text = board?.name ?: "Deckboard",
            fontSize = 12.sp,
            color = Color.White.copy(alpha = 0.75f),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier
                .clip(RoundedCornerShape(14.dp))
                .background(Color.Black.copy(alpha = 0.35f))
                .clickable { menuOpen = true }
                .padding(horizontal = 12.dp, vertical = 6.dp),
        )
        DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
            boards.forEach { b ->
                DropdownMenuItem(
                    text = { Text(b.name ?: "board ${b.id}") },
                    onClick = {
                        menuOpen = false
                        vm.selectBoard(b)
                    },
                )
            }
            DropdownMenuItem(
                text = { Text("Refresh") },
                onClick = {
                    menuOpen = false
                    vm.refresh()
                },
            )
        }
    }
}

@Composable
private fun BoardGrid(vm: DeckboardViewModel, board: Board, modifier: Modifier) {
    val liveValues by vm.customValues.collectAsState()
    val histories by vm.valueHistory.collectAsState()
    // toggle positions are client-side state like in the stock app
    val positions = remember(board.id) { mutableStateMapOf<Long, Int>() }

    BoxWithConstraints(
        modifier
            .background(hex(board.background, DeckColors.background)),
    ) {
        val tile = maxWidth / board.width.coerceAtLeast(1)
        val tileHeight = maxHeight / board.height.coerceAtLeast(1)

        board.shortcuts.filter { it.id != null }.forEach { s ->
            val id = s.id ?: return@forEach
            val pos = positions[id] ?: (s.position ?: 0)
            Box(
                Modifier
                    .offset(x = tile * s.x, y = tileHeight * s.y)
                    .width(tile * s.w)
                    .height(tileHeight * s.h),
            ) {
                // watched key per the original client: extra, then
                // command, then the type itself
                val watchKey = s.extra.ifEmpty { s.command }.ifEmpty { s.type }
                val live = liveValues[watchKey]
                // payloads are scalars ("14:33") or objects ({value, suffix});
                // custom-value tiles show value + suffix, graph tiles read
                // the series from histories
                val liveObj = live as? JsonObject
                val liveText = live?.let { el ->
                    (liveObj?.get("value") ?: el).let { runCatching { it.jsonPrimitive.content }.getOrNull() }
                }
                val liveSuffix = liveObj?.get("suffix")?.let {
                    runCatching { it.jsonPrimitive.content }.getOrNull()
                }
                Tile(
                    shortcut = s,
                    tileSize = tile,
                    customValue = liveText,
                    suffix = liveSuffix,
                    history = histories[watchKey] ?: emptyList(),
                    listItems = listItems(s, live),
                    position = pos,
                    onPressStart = { vm.holdStart(s) },
                    onPressEnd = { vm.holdEnd(s) },
                    onToggle = { positions[id] = if (pos == 1) 0 else 1 },
                    onSlider = { v -> vm.slider(s, v) },
                )
            }
        }
    }
}

