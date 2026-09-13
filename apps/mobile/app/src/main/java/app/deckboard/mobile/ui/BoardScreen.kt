//! Board screen: staggered grid laid out exactly like the stock client -
//! tiles are placed at (x, y) with size (w, h) on a board.width x
//! board.height grid. Live values come from merged APP_CUSTOM_VALUE data.

package app.deckboard.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.sp
import kotlinx.serialization.json.jsonPrimitive
import app.deckboard.mobile.proto.Board
import app.deckboard.mobile.state.DeckboardViewModel

private fun hex(color: String?, fallback: Color): Color =
    color?.let { runCatching { Color(android.graphics.Color.parseColor(it.trim())) }.getOrNull() }
        ?: fallback

@Composable
fun BoardScreen(vm: DeckboardViewModel) {
    val boards by vm.boards.collectAsState()
    val board by vm.currentBoard.collectAsState()

    Column(Modifier.fillMaxSize().background(DeckColors.background)) {
        BoardTopBar(vm, boards, board)
        board?.let { b ->
            BoardGrid(vm, b, Modifier.fillMaxSize())
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun BoardTopBar(vm: DeckboardViewModel, boards: List<Board>, current: Board?) {
    var menuOpen by remember { mutableStateOf(false) }

    TopAppBar(
        title = {
            Column(Modifier.clickable { menuOpen = true }) {
                Text(current?.name ?: "Deckboard", maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    text = "${boards.size} boards",
                    fontSize = 11.sp,
                    color = Color.White.copy(alpha = 0.6f),
                )
            }
        },
        actions = {
            IconButton(onClick = { vm.refresh() }) {
                Icon(Icons.Filled.Refresh, contentDescription = "reconnect")
            }
        },
        colors = TopAppBarDefaults.topAppBarColors(
            containerColor = DeckColors.boardBg,
            titleContentColor = Color.White,
            actionIconContentColor = Color.White,
        ),
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
    }
}

@Composable
private fun BoardGrid(vm: DeckboardViewModel, board: Board, modifier: Modifier) {
    val liveValues by vm.customValues.collectAsState()
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
                Tile(
                    shortcut = s,
                    tileSize = tile,
                    // watched key per the original client: extra, then
                    // command, then the type itself
                    customValue = liveValues[s.extra.ifEmpty { s.command }.ifEmpty { s.type }]?.let {
                        runCatching { it.jsonPrimitive.content }.getOrNull()
                    },
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
